//! Values the plug-in reported, and the session's echoes of them (#651).
//!
//! A value the plug-in reports (an output parameter event, typically an edit in its own editor
//! such as a preset it loads, or a value a `params.rescan` read) goes to the session as an `f32`
//! plain value, and the session sends it back through `set_param`. Sent back as that `f32`, it
//! differs from the plug-in's own `f64` by rounding, and the plug-in calls its preset modified.
//! [`Reported`] keeps each report with the `f32` the session got; [`Echoes`] (the audio side's
//! half) recognizes the session's echo, which the plug-in already has. The VST3 host keeps the
//! same tables (`transit-vst3-host`, `params.rs`).

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Reports kept per parameter: enough for the session's echoes still on their way when the
/// plug-in reports again (a quick run of preset picks, a knob drag in its editor).
const REPORTS_KEPT: usize = 4;

/// What the plug-in reported for each parameter, each with the `f32` value the host handed the
/// session for it. Written on the instance's main thread only (one writer), read on the audio
/// thread with no lock or allocation: a sequence lock per parameter.
pub(crate) struct Reported {
    params: Box<[ReportSlots]>,
}

struct ReportSlots {
    /// Twice the number of reports so far, plus one while one is being written.
    seq: AtomicU64,
    /// Report `k` (from 1) at `(k - 1) % REPORTS_KEPT`: the session's value (`f32` bits) and the
    /// plug-in's (`f64` bits).
    session: [AtomicU32; REPORTS_KEPT],
    plugin: [AtomicU64; REPORTS_KEPT],
}

/// What [`Reported::echo`] found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Echo {
    /// Report number `report` carried this session value, with this value of the plug-in's.
    Of { report: u64, value: f64 },
    /// No report after the ones already seen carried it; `reports` is the number made so far.
    None { reports: u64 },
    /// A report was being written throughout: no answer.
    Busy,
}

impl Reported {
    pub fn new(params: usize) -> Reported {
        Reported {
            params: (0..params)
                .map(|_| ReportSlots {
                    seq: AtomicU64::new(0),
                    session: std::array::from_fn(|_| AtomicU32::new(0)),
                    plugin: std::array::from_fn(|_| AtomicU64::new(0)),
                })
                .collect(),
        }
    }

    /// Records that the plug-in reported `value` for parameter `i`, handed to the session as
    /// `session`. The instance's main thread only (the one writer).
    pub fn record(&self, i: usize, session: f32, value: f64) {
        let Some(slot) = self.params.get(i) else { return };
        let s = slot.seq.load(Ordering::Relaxed);
        slot.seq.store(s.wrapping_add(1), Ordering::Relaxed);
        std::sync::atomic::fence(Ordering::Release);
        let at = (s / 2) as usize % REPORTS_KEPT;
        if let (Some(p), Some(n)) = (slot.session.get(at), slot.plugin.get(at)) {
            p.store(session.to_bits(), Ordering::Relaxed);
            n.store(value.to_bits(), Ordering::Relaxed);
        }
        slot.seq.store(s.wrapping_add(2), Ordering::Release);
    }

    /// The newest report of parameter `i` after report `seen` whose session value is exactly
    /// `session`. Any thread; no lock, no allocation.
    pub fn echo(&self, i: usize, session: f32, seen: u64) -> Echo {
        let Some(slot) = self.params.get(i) else { return Echo::None { reports: 0 } };
        // The writer's section is a few stores: a short bounded retry.
        for _ in 0..4 {
            let s1 = slot.seq.load(Ordering::Acquire);
            if s1 & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let reports = s1 / 2;
            let oldest = seen.saturating_add(1).max(reports.saturating_sub(REPORTS_KEPT as u64 - 1)).max(1);
            let mut found = None;
            let mut k = reports;
            while k >= oldest && found.is_none() {
                let at = ((k - 1) % REPORTS_KEPT as u64) as usize;
                if let (Some(p), Some(n)) = (slot.session.get(at), slot.plugin.get(at))
                    && p.load(Ordering::Relaxed) == session.to_bits()
                {
                    found = Some((k, f64::from_bits(n.load(Ordering::Relaxed))));
                }
                k -= 1;
            }
            std::sync::atomic::fence(Ordering::Acquire);
            if slot.seq.load(Ordering::Relaxed) != s1 {
                continue;
            }
            return match found {
                Some((report, value)) => Echo::Of { report, value },
                None => Echo::None { reports },
            };
        }
        Echo::Busy
    }

    /// The number of reports parameter `i` has had (one being written counts as not yet made).
    fn reports(&self, i: usize) -> u64 {
        self.params.get(i).map_or(0, |s| s.seq.load(Ordering::Acquire) / 2)
    }
}

/// The audio side's half of [`Reported`]: which host-set values are the session's echo of a value
/// the plug-in reported, and the reports already used. Sized at creation; no call allocates.
pub(crate) struct Echoes {
    /// Per parameter: the echoed session value (`f32` bits) and the plug-in's (`f64` bits), while
    /// the host's value is that echo.
    exact: Vec<Option<(u32, u64)>>,
    /// Per parameter: reports up to this number are used up (echoed, or overtaken by a value of
    /// the host's own or by a state load).
    seen: Vec<u64>,
}

impl Echoes {
    pub fn new(params: usize) -> Echoes {
        Echoes { exact: vec![None; params], seen: vec![0; params] }
    }

    /// The host just set `v` for parameter `i`: the session's echo of a report not used up yet,
    /// or a value of the host's own (which uses up every report made so far: one of them coming
    /// back later is then the host's value, not the plug-in's). True when `v` only repeats the
    /// echo the host holds (the session sent the same value again): nothing changes, and a newer
    /// report still on its way stays recognizable.
    pub fn note(&mut self, reported: &Reported, i: usize, v: f32) -> bool {
        let (Some(exact), Some(seen)) = (self.exact.get_mut(i), self.seen.get_mut(i)) else { return false };
        let held = exact.is_some_and(|(session, _)| session == v.to_bits());
        match reported.echo(i, v, *seen) {
            Echo::Of { report, value } => {
                *seen = report;
                *exact = Some((v.to_bits(), value.to_bits()));
                false
            }
            Echo::None { .. } | Echo::Busy if held => true,
            Echo::None { reports } => {
                *seen = reports;
                *exact = None;
                false
            }
            Echo::Busy => {
                *exact = None;
                false
            }
        }
    }

    /// The plug-in's own value when the host's value `v` for parameter `i` is still the session's
    /// echo of it.
    pub fn plugin_value(&self, i: usize, v: f32) -> Option<f64> {
        let (session, x) = (*self.exact.get(i)?)?;
        (session == v.to_bits()).then(|| f64::from_bits(x))
    }

    /// The host read `v` (its own `n`) from the plug-in for parameter `i` (at creation,
    /// or after a state load the host made): that is the plug-in's value, held like an echo, so the
    /// session sending it back is a repeat and nothing goes to the plug-in (#651, round 2).
    pub fn hold(&mut self, i: usize, v: f32, n: f64) {
        if let Some(exact) = self.exact.get_mut(i) {
            *exact = Some((v.to_bits(), n.to_bits()));
        }
    }

    /// A state was loaded: what the plug-in reported before it is history.
    pub fn forget(&mut self, reported: &Reported) {
        self.exact.fill(None);
        for (i, seen) in self.seen.iter_mut().enumerate() {
            *seen = reported.reports(i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The session's echo of a reported value is recognized once, newest report first, even with
    /// later reports in flight; a value of the host's own uses up every report so far, and a state
    /// load forgets them.
    #[test]
    fn an_echo_carries_the_plugins_value_and_a_host_value_uses_up_the_reports() {
        let r = Reported::new(2);
        let mut e = Echoes::new(2);
        let (s03, s07) = (0.3f64 as f32, 0.7f64 as f32);
        r.record(0, s03, 0.3);
        r.record(0, s07, 0.7);
        e.note(&r, 0, s03);
        assert_eq!(e.plugin_value(0, s03).map(f64::to_bits), Some(0.3f64.to_bits()), "the older report, still on its way");
        assert_eq!(e.plugin_value(0, s07), None, "only the value the host holds");
        e.note(&r, 0, s07);
        assert_eq!(e.plugin_value(0, s07).map(f64::to_bits), Some(0.7f64.to_bits()));
        assert_eq!(e.plugin_value(1, s07), None, "per parameter");
        e.note(&r, 0, 1.0);
        assert_eq!(e.plugin_value(0, 1.0), None);
        e.note(&r, 0, s03);
        assert_eq!(e.plugin_value(0, s03), None, "a used report is not an echo again");
        // A report overtaken by a value of the host's own before its echo arrived: the host's.
        r.record(0, s03, 0.3);
        e.note(&r, 0, 1.5);
        e.note(&r, 0, s03);
        assert_eq!(e.plugin_value(0, s03), None, "overtaken by the host's 1.5");
        // Only the last REPORTS_KEPT reports are kept.
        for k in 0..=REPORTS_KEPT {
            r.record(1, k as f32, k as f64 / 10.0);
        }
        e.note(&r, 1, 1.0);
        assert_eq!(e.plugin_value(1, 1.0), Some(0.1), "the oldest report kept");
        let mut other = Echoes::new(2);
        other.note(&r, 1, 0.0);
        assert_eq!(other.plugin_value(1, 0.0), None, "overwritten");
        // A state load forgets what came before it.
        r.record(1, 9.0, 0.9);
        e.forget(&r);
        e.note(&r, 1, 9.0);
        assert_eq!(e.plugin_value(1, 9.0), None);
    }

    /// The session sending the echo the host holds again changes nothing: not a value of the
    /// host's own, and a newer report still on its way stays recognizable (#651, round 2).
    #[test]
    fn a_repeat_of_the_held_echo_changes_nothing() {
        let r = Reported::new(1);
        let mut e = Echoes::new(1);
        let (s03, s07) = (0.3f64 as f32, 0.7f64 as f32);
        r.record(0, s03, 0.3);
        assert!(!e.note(&r, 0, s03), "the echo itself");
        assert!(e.note(&r, 0, s03), "sent again: a repeat");
        assert_eq!(e.plugin_value(0, s03).map(f64::to_bits), Some(0.3f64.to_bits()), "still the plug-in's value");
        r.record(0, s07, 0.7);
        assert!(e.note(&r, 0, s03), "the old value again, with a newer report on its way: a repeat");
        assert!(!e.note(&r, 0, s07), "the newer report's echo");
        assert_eq!(e.plugin_value(0, s07).map(f64::to_bits), Some(0.7f64.to_bits()), "recognized");
        assert!(!e.note(&r, 0, 1.0), "a value of the host's own");
        assert!(!e.note(&r, 0, 1.0), "and again: still the host's, never a repeat of an echo");
        assert_eq!(e.plugin_value(0, 1.0), None);
        assert!(!e.note(&r, 0, s07), "a used report after a host value is the host's");
        assert_eq!(e.plugin_value(0, s07), None);
    }

    #[test]
    fn a_report_being_written_gives_no_answer() {
        let r = Reported::new(1);
        r.record(0, 1.0, 0.5);
        assert_eq!(r.echo(0, 1.0, 0), Echo::Of { report: 1, value: 0.5 });
        r.params[0].seq.fetch_add(1, Ordering::Relaxed);
        assert_eq!(r.echo(0, 1.0, 0), Echo::Busy);
        let mut e = Echoes::new(1);
        e.note(&r, 0, 1.0);
        assert_eq!(e.plugin_value(0, 1.0), None);
        assert_eq!(e.seen[0], 0, "a busy table uses up nothing");
    }
}
