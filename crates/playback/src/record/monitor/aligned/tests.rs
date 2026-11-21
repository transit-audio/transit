//! The aligned read against an independent reference (ADR 0015, Amendment 2026-11-16 on the #709
//! converter term, the acceptance list): a simulated paired device whose capture and output frame
//! counts advance together, an output converter at speed 1 modeled by its exact render positions
//! (`m·S/D` plus an attachment phase), and the reference computed offline as the band-limited reader
//! over the whole captured signal at rule 3's positions `(p − T)·D/S`, never through the ring.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::super::InputRing;
use super::Coordinates;
use crate::band_limited;

/// The eight pairs the ADR lists (session, device) and equal rates.
const PAIRS: [(u32, u32); 9] = [
    (48_000, 44_100),
    (44_100, 48_000),
    (48_000, 96_000),
    (96_000, 48_000),
    (48_000, 88_200),
    (44_100, 88_200),
    (48_000, 176_400),
    (48_000, 192_000),
    (48_000, 48_000),
];

/// A deterministic signal at the device rate, never exactly zero.
fn signal(len: usize) -> Vec<f32> {
    (0..len).map(|i| ((i * 7919 % 2003) as f32 / 1001.5) - 1.0 + 1.0e-3).collect()
}

/// One read the simulation made: the plan frame, the value read, whether the block was aligned and
/// the frames the ring had accepted when it read.
#[derive(Clone, Copy)]
struct Read {
    p: i64,
    value: f32,
    aligned: bool,
    captured: usize,
}

/// A paired device at `device` Hz under a plan at `session` Hz.
struct Sim {
    ring: std::sync::Arc<InputRing>,
    session: u32,
    device: u32,
    term: u32,
    /// The output converter's attachment phase, in units of 1/(1000·device) render frames.
    phase: i64,
    signal: Vec<f32>,
    /// Device frames captured (and output frames consumed): one count, the paired device's.
    ordinal: u64,
    /// The next render block's first plan frame.
    next: i64,
    /// The largest plan block.
    block: usize,
    /// The term the coordinates carry (the ADR's, or one off for a control).
    claimed: u32,
    /// Device frames whose capture never arrived before the frames compared (the reference reads
    /// the delivered frames, so it counts that many fewer).
    lost: i64,
}

impl Sim {
    fn new(session: u32, device: u32, phase: i64, block: usize) -> Self {
        let term = transit_model::converter_term(session, device);
        Sim {
            ring: InputRing::with_pairing(1, device, true),
            session,
            device,
            term,
            phase,
            signal: signal(device as usize),
            ordinal: 0,
            next: 0,
            block,
            claimed: term,
            lost: 0,
        }
    }

    fn q(&self) -> f64 {
        f64::from(self.session) / f64::from(self.device)
    }

    /// The render position of output frame `m`, in units of 1/(1000·device) render frames.
    fn units(&self, m: u64) -> i64 {
        m as i64 * 1000 * i64::from(self.session) + self.phase
    }

    /// One callback of `frames` frames: the capture block (unless `capture` is false: a lost
    /// cycle), then, when `read`, the plan blocks up to the reach of the callback's last frame,
    /// each read with that frame's coordinates (`valid` as given).
    fn callback(&mut self, frames: usize, capture: bool, read: bool, valid: bool) -> Vec<Read> {
        if capture {
            let at = self.ordinal as usize;
            self.ring.push(&self.signal[at..at + frames]);
        }
        let last = self.ordinal + frames as u64 - 1;
        self.ordinal += frames as u64;
        let mut reads = Vec::new();
        if !read {
            return reads;
        }
        let den = 1000 * i64::from(self.device);
        let x = band_limited::position(self.units(last), den);
        let end = band_limited::span(x, self.q()).end;
        while self.next < end {
            let n = ((end - self.next) as usize).min(self.block);
            let at = Coordinates { output: last, units: self.units(last) - self.next * den, den, term: self.claimed, valid };
            let mut out = vec![vec![9.0f32; n]];
            let (aligned, heap) = transit_rt::testing::measure(|| self.ring.pop_aligned(&mut out, n, self.session, at));
            assert!(heap.is_zero(), "the aligned read allocated {heap:?}");
            let captured = self.ring.accepted_frames.load(std::sync::atomic::Ordering::Relaxed) as usize;
            reads.extend(out[0].iter().enumerate().map(|(j, v)| Read { p: self.next + j as i64, value: *v, aligned, captured }));
            self.next += n as i64;
        }
        reads
    }

    /// Rule 3's input position of plan frame `p` with term `t`, in units of 1/(1000·session) device
    /// frames from the first captured frame: render frame `p` is presented at output frame
    /// `(p − φ)/q` (φ the attachment phase in render frames), and read `t/q` frames after it, so
    /// `(p − t − φ)·D/S`; φ·D/S in these units is `phase` itself.
    fn position(&self, p: i64, t: u32) -> i64 {
        (p - i64::from(t)) * 1000 * i64::from(self.device) - self.phase - self.lost * 1000 * i64::from(self.session)
    }

    /// The reference: the reader over the whole signal at rule 3's position, and whether all its
    /// taps lie in the first `captured` frames (else the ring reads counted silence).
    fn reference(&self, p: i64, t: u32, captured: usize) -> (f32, bool) {
        let den = 1000 * i64::from(self.session);
        let q = f64::from(self.device) / f64::from(self.session);
        let x = band_limited::position(self.position(p, t), den);
        let span = band_limited::span(x, q);
        let mut one = [0.0f32];
        band_limited::frame(band_limited::table(), &transit_resample::Mono(&self.signal), x, q, &mut one);
        (one[0], span.start >= 0 && span.end <= captured as i64)
    }
}

/// Compare `reads` with the reference at term `t`: (frames compared, frames that differ, frames of
/// silence where the reference had no taps).
fn compare(sim: &Sim, reads: &[Read], t: u32) -> (usize, usize, usize) {
    let (mut compared, mut differ, mut silent) = (0, 0, 0);
    for r in reads.iter().filter(|r| r.aligned) {
        let (want, available) = sim.reference(r.p, t, r.captured);
        if available {
            compared += 1;
            differ += usize::from(r.value.to_bits() != want.to_bits());
        } else {
            silent += 1;
        }
    }
    (compared, differ, silent)
}

/// Rule 9's start-up bound for the first aligned frame `p0`: ceil(max(0, q·(F + h − y0))) plan
/// frames, F = 0 the first frame kept, y0 its input position, h = halo(1/q).
fn start_bound(sim: &Sim, p0: i64) -> usize {
    let y0 = sim.position(p0, sim.term) as f64 / (1000.0 * f64::from(sim.session));
    let h = transit_resample::halo(f64::from(sim.device) / f64::from(sim.session)) as f64;
    (sim.q() * (h - y0)).max(0.0).ceil() as usize
}

/// Every pair, empty and prefilled rings, several attachment phases, fixed and changing callbacks,
/// blocks split by the plan's largest block: the aligned read is the reference bit for bit wherever
/// the reference's taps were captured, silence is read only while capture starts and within rule 9's
/// bound, and the same reads one frame early or late do not match.
#[test]
fn the_aligned_read_is_the_reader_at_rule_3s_positions_and_one_frame_off_is_not() {
    for (session, device) in PAIRS {
        for (phase_thousandths, prefill, sizes, block) in
            [(0i64, 0usize, &[64usize][..], 512usize), (377, 0, &[100, 37, 256], 64), (0, 7, &[128], 512), (731, 3, &[600, 100], 100)]
        {
            // An equal-rate converter passes frames through only at a whole phase.
            let phase = if session == device { 0 } else { phase_thousandths * i64::from(device) };
            let mut sim = Sim::new(session, device, phase, block);
            let at = format!("{session} on {device}, phase {phase_thousandths}/1000, prefill {prefill}, callbacks {sizes:?}, block {block}");
            // The ring captured before the plan adopted it: output frames passed without a read.
            for k in 0..prefill {
                sim.callback(sizes[k % sizes.len()], true, false, true);
            }
            sim.next = band_limited::span(band_limited::position(sim.units(sim.ordinal), 1000 * i64::from(device)), sim.q()).start.max(0);
            sim.ring.align_on_next_read();
            let mut reads = Vec::new();
            let mut k = 0;
            // At least 4000 plan frames, whatever the rate (60 callbacks of 64 at 192 kHz are
            // fewer than 1000 plan frames at 48 kHz).
            while reads.len() < 4_000 {
                reads.extend(sim.callback(sizes[k % sizes.len()], true, true, true));
                k += 1;
            }
            assert!(reads.iter().all(|r| r.aligned), "{at}: every block aligned");
            let (compared, differ, silent) = compare(&sim, &reads, sim.term);
            assert!(compared > 2_000, "{at}: compared {compared}");
            assert_eq!(differ, 0, "{at}: frames that differ from the reference");
            let bound = start_bound(&sim, reads[0].p);
            assert!(silent <= bound, "{at}: {silent} frames of silence, rule 9's bound {bound}");
            assert_eq!(sim.ring.telemetry().underrun_frames as usize, silent, "{at}: the silence is counted");
            // The silence is all at the start: never once the read has taps.
            let first_read = reads.iter().position(|r| r.value != 0.0).unwrap();
            assert!(reads[first_read..].iter().all(|r| r.value != 0.0), "{at}: silence after the start");
            for wrong in [sim.term + 1, sim.term.saturating_sub(1)].into_iter().filter(|w| *w != sim.term) {
                let (_, differ, _) = compare(&sim, &reads, wrong);
                assert!(differ > 1_000, "{at}: the reads also match the term {wrong}");
            }
        }
    }
}

/// The term (and so the read) one frame early or late through the ring itself: the read then
/// matches the reference at that term, never the ADR's, so the ±1 controls exercise the ring's
/// arithmetic rather than only the reference's.
#[test]
fn coordinates_claiming_a_term_one_off_read_one_frame_off() {
    for (session, device) in [(48_000, 44_100), (44_100, 48_000), (96_000, 48_000)] {
        let term = transit_model::converter_term(session, device);
        for claimed in [term - 1, term + 1] {
            let mut sim = Sim::new(session, device, 0, 512);
            sim.claimed = claimed;
            sim.ring.align_on_next_read();
            let mut reads = Vec::new();
            for _ in 0..40 {
                reads.extend(sim.callback(128, true, true, true));
            }
            let (_, wrong, _) = compare(&sim, &reads, term);
            let (compared, right, _) = compare(&sim, &reads, claimed);
            assert!(wrong > 1_000, "{session} on {device}: a claimed term {claimed} still matches {term}");
            assert!(compared > 1_000 && right == 0, "{session} on {device}: claimed {claimed}: {right} of {compared} differ");
        }
    }
}

/// A capture loss (a cycle whose capture did not arrive, and a push that dropped a partial frame)
/// invalidates the anchor: that callback's reads are the ordinary read, the ring reports Invalid,
/// and the next callback re-anchors from its own pair, after which the read is the reference over
/// the frames actually delivered (a count of accepted frames alone never bridges the loss).
#[test]
fn a_capture_loss_invalidates_the_anchor_until_the_next_callback_re_anchors() {
    for (session, device) in [(48_000, 44_100), (44_100, 48_000), (48_000, 48_000)] {
        let at = format!("{session} on {device}");
        let mut sim = Sim::new(session, device, 0, 512);
        sim.ring.align_on_next_read();
        for _ in 0..20 {
            sim.callback(128, true, true, true);
        }
        assert_eq!(sim.ring.alignment(), transit_model::Alignment::Valid);
        // A lost cycle: the output ran, the capture did not arrive.
        let lost = sim.callback(128, false, true, true);
        assert!(lost.iter().all(|r| !r.aligned), "{at}: the lost cycle's reads are not aligned");
        assert_eq!(sim.ring.alignment(), transit_model::Alignment::Invalid);
        // The next callback re-anchors from its own pair.
        let after: Vec<Read> = (0..30).flat_map(|_| sim.callback(128, true, true, true)).collect();
        assert!(after.iter().all(|r| r.aligned), "{at}: re-anchored at the next pair");
        assert_eq!(sim.ring.alignment(), transit_model::Alignment::Valid);
        // Against the frames delivered: the signal without the lost cycle's 128 frames, read 128
        // frames earlier. Whatever the re-anchor read of the history before the loss differs; from
        // the first frame whose taps are all after it, every frame is the reference.
        let delivered: Vec<f32> = sim.signal[..20 * 128].iter().chain(&sim.signal[21 * 128..]).copied().collect();
        let reference = Sim { signal: delivered, lost: 128, ..Sim::new(session, device, 0, 512) };
        let settled: Vec<Read> =
            after.iter().copied().filter(|r| reference.position(r.p, reference.term) > (20 * 128 + 40) * 1000 * i64::from(session)).collect();
        let (compared, differ, silent) = compare(&reference, &settled, reference.term);
        assert!(compared > 2_000 && differ == 0 && silent == 0, "{at}: {differ} of {compared} differ after the re-anchor, {silent} silent");
    }
    // A partial frame is a loss too.
    let two = InputRing::with_pairing(2, 48_000, true);
    two.push(&[0.1, 0.2, 0.3]);
    assert_eq!(two.losses.load(std::sync::atomic::Ordering::Relaxed), 1, "a partial frame counts as a loss");
}

/// A counted loss invalidates the anchor even when the frame counts still agree (a push dropped by
/// contention, then the cycle's own push): the callback that saw it is not aligned, and the next
/// one re-anchors.
#[test]
fn a_counted_loss_invalidates_the_anchor_even_when_the_counts_agree() {
    let mut sim = Sim::new(48_000, 44_100, 0, 512);
    sim.ring.align_on_next_read();
    for _ in 0..10 {
        sim.callback(128, true, true, true);
    }
    {
        let _held = sim.ring.producer.lock().unwrap();
        sim.ring.push(&[0.5; 64]);
    }
    assert!(sim.callback(128, true, true, true).iter().all(|r| !r.aligned), "the callback that saw the loss");
    assert_eq!(sim.ring.alignment(), transit_model::Alignment::Invalid);
    assert!(sim.callback(128, true, true, true).iter().all(|r| r.aligned), "re-anchored at the next pair");
}

/// Rule 10 at the ring: coordinates whose alignment is not valid read as before (the ring reports
/// Invalid and the block is not aligned); once valid again the read is the reference at once,
/// whatever the ordinary reads left queued (a new alignment from coordinates). A clear re-anchors at
/// once and reads counted silence for the history it discarded, then exact delay.
#[test]
fn an_invalid_alignment_reads_as_before_and_a_valid_one_is_exact_again_after_it_or_a_clear() {
    for (session, device) in [(48_000, 44_100), (96_000, 48_000), (48_000, 48_000)] {
        let mut sim = Sim::new(session, device, 0, 512);
        sim.ring.align_on_next_read();
        for _ in 0..10 {
            sim.callback(256, true, true, true);
        }
        let invalid: Vec<Read> = (0..10).flat_map(|_| sim.callback(256, true, true, false)).collect();
        assert!(invalid.iter().all(|r| !r.aligned));
        assert_eq!(sim.ring.alignment(), transit_model::Alignment::Invalid);
        let back: Vec<Read> = (0..20).flat_map(|_| sim.callback(256, true, true, true)).collect();
        assert!(back.iter().all(|r| r.aligned));
        let (compared, differ, silent) = compare(&sim, &back, sim.term);
        assert!(compared > 2_000 && differ == 0 && silent == 0, "{session} on {device}: after rule 10: {differ} differ, {silent} silent");
        // A clear: the history goes, the anchor is set again at once, silence while the taps are
        // missing, then the reference.
        sim.ring.clear_monitor();
        let cleared: Vec<Read> = (0..20).flat_map(|_| sim.callback(256, true, true, true)).collect();
        assert!(cleared.iter().all(|r| r.aligned));
        let first = cleared.iter().position(|r| r.value != 0.0).unwrap();
        assert!(cleared[first..].iter().all(|r| r.value != 0.0), "{session} on {device}: silence only right after the clear");
        let tail: Vec<Read> = cleared[first..].to_vec();
        let (compared, differ, _) = compare(&sim, &tail, sim.term);
        assert!(compared > 2_000 && differ == 0, "{session} on {device}: after the clear {differ} of {compared} differ");
    }
}

/// An equal-rate phase that is not whole (left by another speed) claims no term; an unpaired ring
/// never aligns and reports no alignment (separate streams are out of the term's scope, rule 1).
#[test]
fn a_fractional_equal_rate_phase_and_an_unpaired_ring_claim_no_term() {
    let mut sim = Sim::new(48_000, 48_000, 500 * 48_000, 512);
    sim.ring.align_on_next_read();
    let reads: Vec<Read> = (0..5).flat_map(|_| sim.callback(64, true, true, true)).collect();
    assert!(reads.iter().all(|r| !r.aligned));
    assert_eq!(sim.ring.alignment(), transit_model::Alignment::Invalid);
    let mut unpaired = Sim::new(48_000, 44_100, 0, 512);
    unpaired.ring = InputRing::new(1, 44_100);
    unpaired.ring.align_on_next_read();
    let reads: Vec<Read> = (0..5).flat_map(|_| unpaired.callback(64, true, true, true)).collect();
    assert!(reads.iter().all(|r| !r.aligned));
    assert_eq!(unpaired.ring.alignment(), transit_model::Alignment::None);
    assert!(!unpaired.ring.paired() && sim.ring.paired());
}
