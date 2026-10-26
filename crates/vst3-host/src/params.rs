//! Parameter plumbing between the host's plain values and VST3's normalized ones, safe to use on
//! the audio thread.
//!
//! VST3 allows the edit controller's conversions (`normalizedParamToPlain`,
//! `plainParamToNormalized`, `getParamStringByValue`) on the UI thread only, while
//! `Plugin::set_param` runs on the audio thread (automation is evaluated per block). So the
//! controller's curve of every plain-mapped parameter is sampled on the UI thread into
//! [`ParamCurves`], and the audio thread converts by interpolating those samples: no controller
//! call, no lock, no allocation. Converted values wait in [`PendingParams`] for the next
//! `process`, which hands them over as `IParameterChanges`.
//!
//! Sampling is lazy and budgeted ([`ParamCurves::step`], from the editor handle's idle): a plug-in
//! with thousands of parameters must not stall creation, so no curve is sampled up front. A curve
//! the audio thread needs before it exists is converted linearly for that block and flagged for
//! priority sampling; the generation bump that ends the sampling makes `process` convert again.
//! A plug-in that changes its curves (`kParamTitlesChanged`) has them all re-sampled the same way,
//! keeping each old curve in use until its replacement is in.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Samples per plain-mapped curve (normalized steps of 1/256). The inverse is piecewise linear, so
/// its error in normalized units is at most `h² / 8 · max|f''/f'|` (h = 1/256): about 1.3e-5 for
/// a 20 Hz–20 kHz exponential curve. 1 KB per plain-mapped parameter.
pub(crate) const CURVE_POINTS: usize = 257;

/// No curve sampled yet (conversions fall back to linear).
const UNSAMPLED: u8 = 0;
/// A finite curve.
const VALID: u8 = 1;
/// Sampled, but the controller gave no finite curve (conversions fall back to linear).
const INVALID: u8 = 2;

/// The sampled normalized → plain curves of the plain-mapped parameters. Written on the UI thread
/// by [`ParamCurves::step`], read on the audio thread; every sample is an atomic, so a reader
/// racing a re-sampling sees a mix of old and new samples for one call, corrected by the
/// generation re-send next block (the [`generation`](ParamCurves::generation) bump that ends the
/// sampling tells the audio thread to convert again).
pub(crate) struct ParamCurves {
    /// Table index of each parameter (same order as the plugin's ids); `None` when the parameter
    /// is not plain-mapped.
    slot: Vec<Option<usize>>,
    /// VST3 id of each table.
    ids: Vec<u32>,
    /// `UNSAMPLED`, `VALID` or `INVALID` per table.
    state: Vec<AtomicU8>,
    /// Tables that need (re-)sampling.
    todo: Vec<AtomicBool>,
    /// Tables the audio thread needed before they were sampled: sampled first.
    want: Vec<AtomicBool>,
    want_any: AtomicBool,
    /// Number of `todo` tables.
    pending: AtomicUsize,
    /// Number of tables that left `UNSAMPLED`.
    sampled: AtomicUsize,
    /// Where the next background pass starts (UI thread only).
    cursor: AtomicUsize,
    /// `CURVE_POINTS` plain values (f32 bits) per table: sample `k` is the plain value at
    /// normalized `k / (CURVE_POINTS - 1)`.
    plain: Vec<AtomicU32>,
    generation: AtomicU64,
}

impl ParamCurves {
    /// Empty tables for the parameters `plain` marks (same order as `ids`). Nothing is sampled
    /// until [`ParamCurves::step`] runs.
    pub fn new(ids: &[u32], plain: &[bool]) -> ParamCurves {
        let mut slot = Vec::with_capacity(ids.len());
        let mut table_ids = Vec::new();
        for (&id, &p) in ids.iter().zip(plain) {
            slot.push(p.then(|| {
                table_ids.push(id);
                table_ids.len() - 1
            }));
        }
        slot.resize(ids.len(), None);
        let tables = table_ids.len();
        ParamCurves {
            slot,
            ids: table_ids,
            state: (0..tables).map(|_| AtomicU8::new(UNSAMPLED)).collect(),
            todo: (0..tables).map(|_| AtomicBool::new(true)).collect(),
            want: (0..tables).map(|_| AtomicBool::new(false)).collect(),
            want_any: AtomicBool::new(false),
            pending: AtomicUsize::new(tables),
            sampled: AtomicUsize::new(0),
            cursor: AtomicUsize::new(0),
            plain: (0..tables * CURVE_POINTS).map(|_| AtomicU32::new(0)).collect(),
            generation: AtomicU64::new(0),
        }
    }

    /// Number of curves sampled at least once.
    pub fn curves_sampled(&self) -> usize {
        self.sampled.load(Ordering::Relaxed)
    }

    /// Number of plain-mapped parameters (curves to sample).
    pub fn curves_total(&self) -> usize {
        self.ids.len()
    }

    /// Marks every curve for re-sampling (the plug-in changed its curves). Each old curve stays in
    /// use until [`ParamCurves::step`] replaces it.
    pub fn invalidate(&self) {
        for f in &self.todo {
            if !f.swap(true, Ordering::Relaxed) {
                self.pending.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.cursor.store(0, Ordering::Relaxed);
    }

    /// Samples curves through `to_plain` (the controller's `normalizedParamToPlain`, so UI thread
    /// only): first those the audio thread asked for, then the others from a rotating cursor,
    /// stopping after `max_tables` curves or once `max_time` has passed (always at least one when
    /// any is due, so progress never stalls). Bumps the generation when it sampled anything and
    /// returns how many curves it sampled.
    pub fn step(&self, max_tables: usize, max_time: Duration, to_plain: impl Fn(u32, f64) -> Option<f64>) -> usize {
        let start = Instant::now();
        let mut done = 0usize;
        let spent = |done: usize| done >= max_tables.max(1) || (done > 0 && start.elapsed() >= max_time);
        let tables = self.ids.len();
        if self.want_any.swap(false, Ordering::Relaxed) {
            for t in 0..tables {
                if !self.want.get(t).is_some_and(|w| w.load(Ordering::Relaxed)) {
                    continue;
                }
                if spent(done) {
                    self.want_any.store(true, Ordering::Relaxed);
                    break;
                }
                self.sample_table(t, &to_plain);
                done += 1;
            }
        }
        if self.pending.load(Ordering::Relaxed) > 0 && tables > 0 {
            let first = self.cursor.load(Ordering::Relaxed) % tables;
            for k in 0..tables {
                let t = (first + k) % tables;
                if !self.todo.get(t).is_some_and(|f| f.load(Ordering::Relaxed)) {
                    continue;
                }
                if spent(done) {
                    self.cursor.store(t, Ordering::Relaxed);
                    break;
                }
                self.sample_table(t, &to_plain);
                done += 1;
            }
        }
        if done > 0 {
            self.generation.fetch_add(1, Ordering::Release);
        }
        done
    }

    fn sample_table(&self, t: usize, to_plain: &impl Fn(u32, f64) -> Option<f64>) {
        let Some(&id) = self.ids.get(t) else { return };
        let mut ok = true;
        for k in 0..CURVE_POINTS {
            let n = k as f64 / (CURVE_POINTS - 1) as f64;
            let v = to_plain(id, n).map(|v| v as f32).filter(|v| v.is_finite());
            ok &= v.is_some();
            if let Some(cell) = self.plain.get(t * CURVE_POINTS + k) {
                cell.store(v.unwrap_or(0.0).to_bits(), Ordering::Relaxed);
            }
        }
        if let Some(s) = self.state.get(t)
            && s.swap(if ok { VALID } else { INVALID }, Ordering::Release) == UNSAMPLED
        {
            self.sampled.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(f) = self.todo.get(t)
            && f.swap(false, Ordering::Relaxed)
        {
            self.pending.fetch_sub(1, Ordering::Relaxed);
        }
        if let Some(w) = self.want.get(t) {
            w.store(false, Ordering::Relaxed);
        }
    }

    /// Changes each time [`ParamCurves::step`] sampled something.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// The samples of parameter `i`'s curve, when it has a valid one. An unsampled curve is
    /// flagged for priority sampling.
    fn table(&self, i: usize) -> Option<&[AtomicU32]> {
        let t = (*self.slot.get(i)?)?;
        match self.state.get(t)?.load(Ordering::Acquire) {
            VALID => self.plain.get(t * CURVE_POINTS..(t + 1) * CURVE_POINTS),
            UNSAMPLED => {
                if let Some(w) = self.want.get(t) {
                    w.store(true, Ordering::Relaxed);
                    self.want_any.store(true, Ordering::Relaxed);
                }
                None
            }
            _ => None,
        }
    }

    /// Normalized → plain for parameter `i` (`None` without a valid curve).
    pub fn to_plain(&self, i: usize, n: f64) -> Option<f64> {
        let table = self.table(i)?;
        let at = |k: usize| table.get(k).map_or(0.0, |c| f64::from(f32::from_bits(c.load(Ordering::Relaxed))));
        let x = n.clamp(0.0, 1.0) * (CURVE_POINTS - 1) as f64;
        let k = (x.floor() as usize).min(CURVE_POINTS - 2);
        let frac = x - k as f64;
        Some(at(k) + frac * (at(k + 1) - at(k)))
    }

    /// Plain → normalized for parameter `i`, clamped to 0..1 (`None` without a valid curve). The
    /// curve may rise or fall; a curve that does neither gives some in-range value.
    pub fn to_normalized(&self, i: usize, plain: f64) -> Option<f64> {
        let table = self.table(i)?;
        if !plain.is_finite() {
            return None;
        }
        let at = |k: usize| table.get(k).map_or(0.0, |c| f64::from(f32::from_bits(c.load(Ordering::Relaxed))));
        let last = CURVE_POINTS - 1;
        // Work on a rising curve: flip the sign of a falling one.
        let sign = if at(last) >= at(0) { 1.0 } else { -1.0 };
        let key = |k: usize| sign * at(k);
        let v = sign * plain;
        if v <= key(0) {
            return Some(0.0);
        }
        if v >= key(last) {
            return Some(1.0);
        }
        // Largest k with key(k) <= v, in 0..last.
        let (mut lo, mut hi) = (0usize, last);
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            if key(mid) <= v {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let (a, b) = (key(lo), key(lo + 1));
        let frac = if b > a { ((v - a) / (b - a)).clamp(0.0, 1.0) } else { 0.0 };
        let n = (lo as f64 + frac) / last as f64;
        n.is_finite().then(|| n.clamp(0.0, 1.0))
    }
}

/// Normalized values waiting for the next `process`: one slot per parameter, so the latest value
/// of each parameter wins and nothing is ever dropped, and no call allocates (all storage is
/// sized at creation). Parameters leave in the order they first changed; a block takes at most
/// as many as the `IParameterChanges` pool holds and the rest go in the next block.
pub(crate) struct PendingParams {
    value: Vec<f64>,
    queued: Vec<bool>,
    /// Indices of queued parameters, oldest first; never longer than the parameter count.
    order: Vec<usize>,
}

impl PendingParams {
    pub fn new(params: usize) -> PendingParams {
        PendingParams { value: vec![0.0; params], queued: vec![false; params], order: Vec::with_capacity(params) }
    }

    /// Queues normalized `n` for parameter `i`, replacing a value still waiting.
    pub fn set(&mut self, i: usize, n: f64) {
        let (Some(v), Some(q)) = (self.value.get_mut(i), self.queued.get_mut(i)) else { return };
        *v = n;
        if !*q && self.order.len() < self.order.capacity() {
            *q = true;
            self.order.push(i);
        }
    }

    pub fn clear(&mut self) {
        for &i in &self.order {
            if let Some(q) = self.queued.get_mut(i) {
                *q = false;
            }
        }
        self.order.clear();
    }

    /// Moves up to `max` waiting values into `out` as (VST3 id, normalized), oldest first, never
    /// past `out`'s capacity.
    pub fn take_into(&mut self, ids: &[u32], out: &mut Vec<(u32, f64)>, max: usize) {
        let k = self.order.len().min(max).min(out.capacity().saturating_sub(out.len()));
        for &i in self.order.get(..k).unwrap_or(&[]) {
            if let (Some(&id), Some(&v)) = (ids.get(i), self.value.get(i)) {
                out.push((id, v));
            }
            if let Some(q) = self.queued.get_mut(i) {
                *q = false;
            }
        }
        self.order.drain(..k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Samples every curve at once.
    fn full(c: &ParamCurves, f: impl Fn(u32, f64) -> Option<f64>) {
        c.step(usize::MAX, Duration::MAX, f);
    }

    fn exp_curve(_id: u32, n: f64) -> Option<f64> {
        Some(20.0 * 1000f64.powf(n))
    }

    #[test]
    fn a_sampled_log_curve_puts_mid_travel_at_the_geometric_mean() {
        let c = ParamCurves::new(&[40, 41], &[true, true]);
        assert_eq!(c.to_plain(0, 0.5), None, "unsampled: no curve yet");
        assert_eq!(c.to_normalized(0, 440.0), None, "unsampled: no curve yet");
        full(&c, |id, n| (id == 40).then(|| 13.6 * (14_700.0f64 / 13.6).powf(n)));
        let mid = c.to_plain(0, 0.5).unwrap();
        let geometric = (13.6f64 * 14_700.0).sqrt();
        assert!((mid - geometric).abs() < 1e-3 * geometric, "{mid} vs {geometric}");
        let back = c.to_normalized(0, mid).unwrap();
        assert!((back - 0.5).abs() < 1e-3, "{back}");
        assert!(c.to_plain(0, 1.0 / 127.0).unwrap() < 20.0, "the first CC step stays near the bottom");
        assert_eq!(c.to_plain(1, 0.5), None, "a parameter whose sampling gave nothing has no curve");
    }

    #[test]
    fn curves_match_the_controller_within_1e_4() {
        let c = ParamCurves::new(&[5, 12, 7], &[false, true, true]);
        assert_eq!(c.to_normalized(1, 100.0), None, "empty until sampled");
        let g = c.generation();
        full(&c, |id, n| match id {
            12 => exp_curve(id, n),
            7 => Some(2.0 - 2.0 * n),
            _ => None,
        });
        assert_ne!(c.generation(), g);
        assert_eq!(c.to_normalized(0, 1.0), None, "not plain-mapped");
        for v in [20.0, 25.0, 100.0, 440.0, 1000.0, 5_000.0, 19_999.0, 20_000.0] {
            let want = (v / 20.0f64).ln() / 1000f64.ln();
            let got = c.to_normalized(1, v).unwrap();
            assert!((got - want).abs() < 1e-4, "{v}: {got} vs {want}");
            let back = c.to_plain(1, got).unwrap();
            assert!((back - v).abs() <= 1e-4 * v, "round trip {v}: {back}");
        }
        assert_eq!(c.to_normalized(1, 1.0), Some(0.0), "below the range clamps");
        assert_eq!(c.to_normalized(1, 1e9), Some(1.0), "above the range clamps");
        // A falling curve.
        for v in [0.0, 0.3, 1.0, 1.7, 2.0] {
            let got = c.to_normalized(2, v).unwrap();
            assert!((got - (2.0 - v) / 2.0).abs() < 1e-6, "{v}: {got}");
        }
        assert_eq!(c.to_normalized(1, f64::NAN), None);
    }

    #[test]
    fn a_curve_with_holes_falls_back() {
        let c = ParamCurves::new(&[1], &[true]);
        full(&c, |_, n| (n < 0.5).then_some(n));
        assert_eq!(c.to_plain(0, 0.25), None);
        c.invalidate();
        full(&c, |_, n| Some(n));
        assert!((c.to_plain(0, 0.25).unwrap() - 0.25).abs() < 1e-6, "a later sampling restores it");
    }

    #[test]
    fn sampling_is_lazy_and_budgeted_per_step() {
        let ids: Vec<u32> = (0..300).collect();
        let c = ParamCurves::new(&ids, &vec![true; ids.len()]);
        let calls = std::cell::Cell::new(0usize);
        let f = |_: u32, n: f64| {
            calls.set(calls.get() + 1);
            Some(n)
        };
        assert_eq!((c.curves_sampled(), c.curves_total(), calls.get()), (0, 300, 0), "creation samples nothing");
        let mut frames = 0;
        while c.curves_sampled() < c.curves_total() {
            let before = calls.get();
            let n = c.step(64, Duration::MAX, f);
            assert_eq!(calls.get() - before, n * CURVE_POINTS);
            assert!(calls.get() - before <= 64 * CURVE_POINTS, "a frame samples at most 64 curves");
            frames += 1;
            assert!(frames <= 5);
        }
        assert_eq!((frames, calls.get()), (5, 300 * CURVE_POINTS));
        assert_eq!(c.step(64, Duration::MAX, f), 0, "nothing left to do");
    }

    #[test]
    fn a_time_budget_still_makes_progress() {
        let c = ParamCurves::new(&[1, 2, 3], &[true; 3]);
        assert_eq!(c.step(64, Duration::ZERO, |_, n| Some(n)), 1, "one curve per step at least");
        assert_eq!(c.curves_sampled(), 1);
    }

    #[test]
    fn the_audio_thread_falls_back_and_asks_for_priority_sampling() {
        let ids: Vec<u32> = (0..200).collect();
        let c = ParamCurves::new(&ids, &vec![true; ids.len()]);
        let g = c.generation();
        assert_eq!(c.to_plain(190, 0.5), None, "unsampled: the caller converts linearly this block");
        assert_eq!(c.to_normalized(7, 1.0), None);
        assert_eq!(c.step(2, Duration::MAX, |_, n| Some(2.0 * n)), 2);
        assert_eq!(c.curves_sampled(), 2);
        assert!(c.to_plain(190, 0.5).is_some() && c.to_plain(7, 0.5).is_some(), "the touched curves went first");
        assert!(c.to_plain(0, 0.5).is_none(), "the cursor pass has not reached 0 yet");
        assert_ne!(c.generation(), g, "the generation bump re-sends the value");
    }

    #[test]
    fn invalidate_keeps_the_old_curve_until_its_replacement_is_in() {
        let c = ParamCurves::new(&[1, 2], &[true; 2]);
        full(&c, |_, n| Some(n));
        let g = c.generation();
        c.invalidate();
        assert!((c.to_plain(0, 0.5).unwrap() - 0.5).abs() < 1e-6, "old curve still used");
        assert_eq!(c.step(1, Duration::MAX, |_, n| Some(10.0 * n)), 1);
        assert!((c.to_plain(0, 0.5).unwrap() - 5.0).abs() < 1e-5);
        assert!((c.to_plain(1, 0.5).unwrap() - 0.5).abs() < 1e-6, "second not re-sampled yet");
        assert_eq!(c.step(1, Duration::MAX, |_, n| Some(10.0 * n)), 1);
        assert!((c.to_plain(1, 0.5).unwrap() - 5.0).abs() < 1e-5);
        assert_eq!((c.curves_sampled(), c.step(8, Duration::MAX, |_, n| Some(n))), (2, 0));
        assert_ne!(c.generation(), g);
    }

    #[test]
    fn pending_keeps_the_latest_value_per_parameter_and_never_drops_or_grows() {
        let ids: Vec<u32> = (100..110).collect();
        let mut p = PendingParams::new(ids.len());
        let cap = p.order.capacity();
        let ptr = p.order.as_ptr();
        // Far more changes than parameters: every parameter changes many times.
        for round in 0..50 {
            for i in 0..ids.len() {
                p.set(i, f64::from(round) + i as f64 / 100.0);
            }
        }
        p.set(99, 1.0); // out of range: ignored
        assert_eq!(p.order.len(), ids.len(), "one slot per parameter");
        assert_eq!((p.order.capacity(), p.order.as_ptr()), (cap, ptr), "no reallocation");
        // A pool smaller than the backlog: the rest carries over, in first-changed order.
        let mut out = Vec::with_capacity(4);
        p.take_into(&ids, &mut out, usize::MAX);
        assert_eq!(out, vec![(100, 49.0), (101, 49.01), (102, 49.02), (103, 49.03)]);
        assert_eq!(p.order.len(), 6);
        p.set(0, 7.0);
        out.clear();
        p.take_into(&ids, &mut out, 3);
        assert_eq!(out, vec![(104, 49.04), (105, 49.05), (106, 49.06)]);
        out.clear();
        p.take_into(&ids, &mut out, usize::MAX);
        assert_eq!(out, vec![(107, 49.07), (108, 49.08), (109, 49.09), (100, 7.0)]);
        assert_eq!(p.order.len(), 0);
        p.set(3, 0.5);
        p.clear();
        out.clear();
        p.take_into(&ids, &mut out, usize::MAX);
        assert!(out.is_empty());
        p.set(3, 0.25);
        p.take_into(&ids, &mut out, usize::MAX);
        assert_eq!(out, vec![(103, 0.25)], "cleared slots queue again");
    }
}
