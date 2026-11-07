//! The live integrated loudness (#366) against the bounce's offline integrator
//! (`meter::LoudnessMeter`): the same signal must read within 0.1 LU, the gates must behave as
//! BS.1770-4 says, the histogram's range must hold, and the audio-thread path must not allocate.
//!
//! The binary installs `transit_rt::testing::CountingAlloc` and counts the test thread only.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::f32::consts::TAU;
use transit_dsp::db_to_gain;
use transit_dsp::loudness::{CHANNELS, HIST_BINS, LiveLoudness};
use transit_dsp::meter::LoudnessMeter;
use transit_rt::testing::{CountingAlloc, measure};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

const SR: f32 = 48_000.0;
/// The bound the brief sets for the live figure against the reference.
const MATCH_LU: f32 = 0.1;

fn sine(freq: f32, amp: f32, len: usize) -> Vec<f32> {
    (0..len).map(|i| amp * (TAU * freq * i as f32 / SR).sin()).collect()
}

/// Deterministic pink noise (a linear congruential white source through Paul Kellet's economy
/// filter), scaled to roughly `amp` peak.
fn pink(seed: u32, amp: f32, len: usize) -> Vec<f32> {
    let mut state = seed.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let (mut b0, mut b1, mut b2) = (0.0f32, 0.0f32, 0.0f32);
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let w = (state >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
            b0 = 0.997_65 * b0 + w * 0.099_046;
            b1 = 0.963 * b1 + w * 0.296_516_4;
            b2 = 0.57 * b2 + w * 1.052_691_3;
            (b0 + b1 + b2 + w * 0.1848) * amp * 0.25
        })
        .collect()
}

fn silence(len: usize) -> Vec<f32> {
    vec![0.0; len]
}

fn cat(parts: &[Vec<f32>]) -> Vec<f32> {
    parts.concat()
}

/// Feeds both meters in device-sized blocks (an odd size, so blocks straddle the 100 ms
/// sub-blocks) and returns (live, offline). The offline meter's "no figure" is `MIN_DB`.
fn both(io: &[Vec<f32>]) -> (Option<f32>, f32) {
    let frames = io[0].len();
    let mut live = LiveLoudness::new(SR, io.len());
    let mut offline = LoudnessMeter::new(SR, io.len());
    offline.process(io, frames);
    let mut block = vec![Vec::with_capacity(333); io.len()];
    let mut at = 0;
    while at < frames {
        let n = 333.min(frames - at);
        for (b, c) in block.iter_mut().zip(io) {
            b.clear();
            b.extend_from_slice(&c[at..at + n]);
        }
        live.process(&block, n);
        at += n;
    }
    (live.integrated_lufs(), offline.integrated_lufs())
}

fn assert_matches(what: &str, io: &[Vec<f32>]) -> f32 {
    let (live, offline) = both(io);
    let live = live.unwrap_or_else(|| panic!("{what}: no live figure"));
    let diff = (live - offline).abs();
    eprintln!("{what}: live {live:.4} LUFS, offline {offline:.4} LUFS, |diff| {diff:.4} LU");
    assert!(diff <= MATCH_LU, "{what}: live {live} offline {offline}");
    live
}

#[test]
fn a_pink_noise_burst_reads_as_the_offline_integrator_does() {
    let n = SR as usize;
    let (l, r) = (pink(1, 0.25, 6 * n), pink(2, 0.25, 6 * n));
    let io = [cat(&[silence(2 * n), l, silence(2 * n)]), cat(&[silence(2 * n), r, silence(2 * n)])];
    let live = assert_matches("pink burst", &io);
    assert!(live < -10.0 && live > -40.0, "a sensible level: {live}");
}

#[test]
fn a_minus_20_dbfs_sine_reads_minus_20_as_the_offline_integrator_does() {
    let x = sine(997.0, db_to_gain(-20.0), 10 * SR as usize);
    let live = assert_matches("-20 dBFS 997 Hz stereo", &[x.clone(), x]);
    assert!((live + 20.0).abs() < 0.1, "{live}");
}

#[test]
fn a_silence_stretch_does_not_lower_the_figure() {
    let n = SR as usize;
    let tone = sine(997.0, db_to_gain(-20.0), 5 * n);
    let short = cat(&[tone.clone(), silence(n)]);
    let long = cat(&[tone, silence(30 * n)]);
    let (a, _) = both(&[short.clone(), short]);
    let (b, _) = both(&[long.clone(), long.clone()]);
    assert_eq!(a, b, "29 more seconds of silence change nothing once the silence fills the window");
    assert_matches("tone then 30 s of silence", &[long.clone(), long]);
}

#[test]
fn a_quiet_tail_below_the_relative_gate_is_excluded_and_one_above_it_is_not() {
    let n = SR as usize;
    let loud = sine(997.0, db_to_gain(-20.0), 5 * n);
    let (alone, _) = both(&[loud.clone(), loud.clone()]);
    let alone = alone.unwrap();
    // A tail 20 LU down, six times as long as the tone: below the relative gate.
    let quiet = cat(&[loud.clone(), sine(997.0, db_to_gain(-40.0), 30 * n)]);
    let gated = assert_matches("tone with a tail 20 LU down", &[quiet.clone(), quiet]);
    assert!((gated - alone).abs() < 0.2, "the tail is gated out: {gated} against {alone}");
    // Control: a tail only 5 LU down is above the gate and pulls the figure toward it.
    let near = cat(&[loud, sine(997.0, db_to_gain(-25.0), 30 * n)]);
    let kept = assert_matches("tone with a tail 5 LU down", &[near.clone(), near]);
    assert!(kept < alone - 2.0, "the near tail counts: {kept} against {alone}");
}

#[test]
fn no_figure_before_the_first_gated_block() {
    let mut live = LiveLoudness::new(SR, 2);
    assert_eq!(live.integrated_lufs(), None, "nothing measured");
    // 390 ms of a loud tone: no 400 ms block has finished.
    let x = sine(997.0, 0.5, (SR * 0.39) as usize);
    live.process(&[x.clone(), x.clone()], x.len());
    assert_eq!(live.integrated_lufs(), None, "under 400 ms");
    // Control: 20 ms more finishes the first block.
    let more = sine(997.0, 0.5, (SR * 0.02) as usize);
    live.process(&[more.clone(), more], (SR * 0.02) as usize);
    assert!(live.integrated_lufs().is_some());
    live.reset();
    assert_eq!(live.integrated_lufs(), None, "a reset clears the figure");
    // Ten seconds of silence never pass the absolute gate.
    let quiet = silence(10 * SR as usize);
    live.process(&[quiet.clone(), quiet], 10 * SR as usize);
    assert_eq!((live.integrated_lufs(), live.gated_blocks()), (None, 0));
}

#[test]
fn the_histogram_covers_the_absolute_gate_to_plus_10_and_clamps_louder_blocks_exactly() {
    assert_eq!(HIST_BINS, 800, "0.1 LU bins over -70 ..= +10 LUFS");
    // A mono 997 Hz sine reads 3.01 LU under its stereo pair: -66.99 dBFS mono is about -70 LUFS.
    let at = |dbfs: f32| {
        let x = sine(997.0, db_to_gain(dbfs), 3 * SR as usize);
        both(&[x])
    };
    let (above, _) = at(-66.0);
    assert!(above.is_some_and(|l| (l + 69.0).abs() < 0.2), "a block 1 LU above the absolute gate counts: {above:?}");
    let (below, offline) = at(-68.0);
    assert_eq!(below, None, "1 LU below the gate does not");
    assert!(offline < -100.0, "nor offline: {offline}");
    // Sixteen full-scale 8 kHz channels read about +13 LUFS, past the top bin: the energy is kept.
    let x = sine(8_000.0, 1.0, 2 * SR as usize);
    let io = vec![x; CHANNELS];
    let live = assert_matches("16 full-scale channels", &io);
    assert!(live > 10.0, "above the histogram's top: {live}");
}

#[test]
fn the_audio_thread_path_allocates_nothing_and_the_state_is_fixed_size() {
    let x = pink(3, 0.5, 4_800);
    let io = [x.clone(), x];
    let (mut live, heap) = measure(|| LiveLoudness::new(SR, 2));
    assert!(heap.is_zero(), "new: {heap:?}");
    for _ in 0..50 {
        let ((), heap) = measure(|| live.process(&io, 4_800));
        assert!(heap.is_zero(), "process: {heap:?}");
    }
    assert!(live.integrated_lufs().is_some());
    let ((), heap) = measure(|| {
        live.set_channels(6);
        live.reset();
        live.set_channels(2);
    });
    assert!(heap.is_zero(), "reset: {heap:?}");
    let size = std::mem::size_of::<LiveLoudness>();
    eprintln!("LiveLoudness state: {size} bytes");
    assert!(size < 12 * 1024, "{size} bytes");
}
