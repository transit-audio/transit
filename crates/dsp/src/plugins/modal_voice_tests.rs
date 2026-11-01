//! The Modal Voice instrument (#198 slice 2): at a 32 kHz session (the engine's own output, nothing
//! converted) the instrument driven by notes is the crate driven directly with the same gate, note
//! and strength per block, bit for bit; host-block independence at 44.1, 48 and 96 kHz; the edge's
//! reported latency against the delay it has; a held A4's pitch at every session rate (E63); the
//! gate rules (retrigger, legato, the pedal, CC 120 and 123); velocity as strength; the matrix on
//! the part's controls; the shaper's held-gate switch (#262); reset.

use super::*;
use crate::Plugin;
use std::f64::consts::TAU;

fn instrument(sr: u32, params: &[(&str, f32)]) -> ModalVoiceInstrument {
    let mut r = ModalVoiceInstrument::new();
    for (id, v) in params {
        assert!(r.set_param(id, *v), "{id}");
    }
    r.prepare(sr as f32, 512, 2);
    r
}

#[derive(Debug, Clone, Copy)]
enum Ev {
    On(u8, u8),
    Off(u8),
    Param(&'static str, f32),
    Cc(u8, u8),
    Bend(u16),
}

/// Renders `frames` in host blocks of `block`; each event is delivered in the host block holding
/// its frame, at its offset (a parameter at the block's start: script them on frames both splits
/// start a block at).
fn render(r: &mut ModalVoiceInstrument, frames: usize, block: usize, script: &[(usize, Ev)]) -> [Vec<f32>; 2] {
    let mut out = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
    let mut io = vec![vec![0.0f32; block]; 2];
    let mut pos = 0;
    while pos < frames {
        let n = block.min(frames - pos);
        for (at, ev) in script.iter().filter(|(at, _)| (pos..pos + n).contains(at)) {
            let offset = at - pos;
            match *ev {
                Ev::On(key, vel) => r.note_on(offset, key, vel),
                Ev::Off(key) => r.note_off(offset, key),
                Ev::Param(id, v) => assert!(r.set_param(id, v), "{id}"),
                Ev::Cc(cc, v) => r.midi(offset, [0xB0, cc, v]),
                Ev::Bend(v) => r.midi(offset, [0xE0, (v & 0x7F) as u8, (v >> 7) as u8]),
            }
        }
        r.process(&mut io, n);
        for c in 0..2 {
            out[c].extend_from_slice(&io[c][..n]);
        }
        pos += n;
    }
    out
}

fn same_bits(a: &[Vec<f32>; 2], b: &[Vec<f32>; 2]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.len() == y.len() && x.iter().zip(y).all(|(p, q)| p.to_bits() == q.to_bits()))
}

fn same_from(a: &[f32], b: &[f32], from: usize, to: usize) -> bool {
    a[from..to].iter().zip(&b[from..to]).all(|(x, y)| x.to_bits() == y.to_bits())
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

/// A held note, a second note while it is held (a retrigger), releases back to the first (legato)
/// and to nothing, a new note, at offsets on and off the 16-frame grid.
const NOTES: [(usize, Ev); 7] = [
    (0, Ev::On(69, 127)),
    (1000, Ev::On(72, 90)),
    (3000, Ev::Off(72)),
    (5003, Ev::Off(69)),
    (8000, Ev::On(60, 64)),
    (12_001, Ev::Off(60)),
    (20_000, Ev::On(48, 110)),
];

/// The part blocks where the gate, the note or the strength changes for [`NOTES`] at 32 kHz (each
/// event takes the first 16-frame block starting at or after its frame): (block, gate, key,
/// velocity). The second note drops the gate for block 63 and raises it at block 64; releasing it
/// moves back to 69 with the gate up.
const SCHEDULE: [(usize, bool, u8, u8); 8] = [
    (0, true, 69, 127),
    (63, false, 72, 90),
    (64, true, 72, 90),
    (188, true, 69, 90),
    (313, false, 69, 90),
    (500, true, 60, 64),
    (751, false, 60, 64),
    (1250, true, 48, 110),
];

/// The crate driven directly in 16-frame blocks, as the instrument should drive it: the model
/// choice, the patch, and per block the gate, the note and the strength of [`SCHEDULE`]-like
/// changes. Returns (left, right) = (the part's aux, its main).
fn crate_render(model: usize, patch: Patch, fm: f32, velocity_amount: f32, schedule: &[(usize, bool, u8, u8)], blocks: usize) -> [Vec<f32>; 2] {
    let mut part = ModalVoice::new();
    if model == 3 {
        part.set_easter_egg(true);
    } else {
        part.set_model(ResonatorModel::ALL[model]);
    }
    *part.patch_mut() = patch;
    let (mut gate, mut key, mut vel) = (false, 60u8, 127u8);
    let mut sent = FM_SHUT;
    let mut out = [Vec::new(), Vec::new()];
    for b in 0..blocks {
        if let Some((_, g, k, v)) = schedule.iter().find(|(at, ..)| *at == b) {
            (gate, key, vel) = (*g, *k, *v);
        }
        let performance = PerformanceState {
            gate,
            note: f32::from(key),
            modulation: fm,
            strength: gated_strength(&mut sent, gate, model == 3, strength_of(f32::from(vel) / 127.0, velocity_amount)),
        };
        let (mut m, mut a) = ([0.0f32; BLOCK], [0.0f32; BLOCK]);
        part.process(&performance, &[0.0; BLOCK], &[0.0; BLOCK], &mut m, &mut a);
        out[0].extend_from_slice(&a);
        out[1].extend_from_slice(&m);
    }
    out
}

/// The patch the instrument's parameters below set (each knob over 100, capped at the module's
/// ceilings).
fn patch_of(bow: f32, blow: f32, strike: f32) -> Patch {
    Patch {
        exciter_envelope_shape: 0.45,
        exciter_bow_level: bow,
        exciter_bow_timbre: 0.6,
        exciter_blow_level: blow,
        exciter_blow_meta: 0.3,
        exciter_blow_timbre: 0.7,
        exciter_strike_level: strike,
        exciter_strike_meta: 0.4,
        exciter_strike_timbre: 0.55,
        resonator_geometry: 0.35,
        resonator_brightness: 0.6,
        resonator_damping: 0.7,
        resonator_position: 0.3,
        space: 0.8,
    }
}

fn params_of(model: usize, p: &Patch) -> Vec<(&'static str, f32)> {
    vec![
        ("model", model as f32),
        ("contour", p.exciter_envelope_shape * 100.0),
        ("bow_level", p.exciter_bow_level * 100.0),
        ("bow_timbre", p.exciter_bow_timbre * 100.0),
        ("blow_level", p.exciter_blow_level * 100.0),
        ("flow", p.exciter_blow_meta * 100.0),
        ("blow_timbre", p.exciter_blow_timbre * 100.0),
        ("strike_level", p.exciter_strike_level * 100.0),
        ("mallet", p.exciter_strike_meta * 100.0),
        ("strike_timbre", p.exciter_strike_timbre * 100.0),
        ("geometry", p.resonator_geometry * 100.0),
        ("brightness", p.resonator_brightness * 100.0),
        ("damping", p.resonator_damping * 100.0),
        ("position", p.resonator_position * 100.0),
        ("space", p.space * 100.0),
        ("level", 0.0),
    ]
}

/// The knob values as the parameters store them and the instrument divides them: what the crate
/// must be given for an exact comparison.
fn as_stored(p: &Patch) -> Patch {
    let back = |x: f32| (x * 100.0) / 100.0;
    Patch {
        exciter_envelope_shape: back(p.exciter_envelope_shape),
        exciter_bow_level: back(p.exciter_bow_level),
        exciter_bow_timbre: back(p.exciter_bow_timbre),
        exciter_blow_level: back(p.exciter_blow_level),
        exciter_blow_meta: back(p.exciter_blow_meta),
        exciter_blow_timbre: back(p.exciter_blow_timbre),
        exciter_strike_level: back(p.exciter_strike_level),
        exciter_strike_meta: back(p.exciter_strike_meta),
        exciter_strike_timbre: back(p.exciter_strike_timbre),
        resonator_geometry: back(p.resonator_geometry),
        resonator_brightness: back(p.resonator_brightness),
        resonator_damping: back(p.resonator_damping),
        resonator_position: back(p.resonator_position),
        space: back(p.space),
    }
}

/// At a 32 kHz session nothing is converted and the latency is 0: the instrument's left and right
/// outputs are the crate's aux and main, bit for bit, for every model (the FM pair included), in
/// host blocks of 64 and 500 frames (level 0 dB, the shaper off, no matrix row; tune through the FM
/// input; Velocity at 100% and at 40%).
#[test]
fn at_32_khz_the_instrument_is_the_crate_driven_directly() {
    let blocks = 1600;
    let frames = blocks * BLOCK;
    for model in 0..4 {
        let patch = as_stored(&patch_of(0.5, 0.4, 0.8));
        let tune = if model % 2 == 0 { 0.0 } else { 3.0 };
        let velocity = if model < 2 { 100.0 } else { 40.0 };
        let want = crate_render(model, patch, tune, velocity / 100.0, &SCHEDULE, blocks);
        assert!(peak(&want[0]) > 0.01 && peak(&want[1]) > 0.01, "model {model}: the reference is silent");
        let mut params = params_of(model, &patch_of(0.5, 0.4, 0.8));
        params.push(("tune", tune));
        params.push(("velocity", velocity));
        for block in [64, 500] {
            let mut r = instrument(32_000, &params);
            assert_eq!(r.latency(), 0);
            let got = render(&mut r, frames, block, &NOTES);
            assert!(same_bits(&got, &want), "model {model}, host block {block}: not the crate's bits");
        }
    }
}

/// Velocity is the strength input: `1 - amount · (1 - velocity)`.
#[test]
fn velocity_is_the_strength_input() {
    assert_eq!(strength_of(0.5, 1.0).to_bits(), 0.5f32.to_bits());
    assert_eq!(strength_of(0.25, 0.0).to_bits(), 1.0f32.to_bits());
    assert!((strength_of(0.5, 0.4) - 0.8).abs() < 1e-7);
    assert_eq!(strength_of(f32::NAN.max(0.0), 1.0).to_bits(), 0.0f32.to_bits());
}

/// The pedal holds the gate: a note released under the pedal plays on exactly as a held one, until
/// the pedal lifts, which lowers the gate as the note-off would have there.
#[test]
fn the_pedal_holds_the_gate() {
    let params = params_of(0, &patch_of(0.6, 0.0, 0.5));
    let held = render(&mut instrument(32_000, &params), 16_000, 256, &[(0, Ev::On(57, 100)), (9600, Ev::Off(57))]);
    let pedalled = render(
        &mut instrument(32_000, &params),
        16_000,
        256,
        &[(0, Ev::Cc(64, 127)), (0, Ev::On(57, 100)), (3200, Ev::Off(57)), (9600, Ev::Cc(64, 0))],
    );
    assert!(same_bits(&held, &pedalled), "the pedal held the gate up to its lift, which lowered it");
}

/// CC 123 lowers the gate as a note-off does; CC 120 silences at once (from the next block on).
#[test]
fn all_notes_off_lowers_the_gate_and_all_sound_off_silences() {
    let params = params_of(0, &patch_of(0.6, 0.3, 0.8));
    let off = render(&mut instrument(32_000, &params), 16_000, 256, &[(0, Ev::On(57, 100)), (4800, Ev::Off(57))]);
    let cc123 = render(&mut instrument(32_000, &params), 16_000, 256, &[(0, Ev::On(57, 100)), (4800, Ev::Cc(123, 0))]);
    assert!(same_bits(&off, &cc123), "All Notes Off is the note-off");
    let cc120 = render(&mut instrument(32_000, &params), 16_000, 256, &[(0, Ev::On(57, 100)), (4800, Ev::Cc(120, 0))]);
    assert!(same_from(&cc120[0], &off[0], 0, 4800), "nothing changes before the CC");
    assert!(peak(&off[0][4800 + 32..]) > 0.001, "the released note rings on");
    assert_eq!(peak(&cc120[0][4800 + 32..]), 0.0, "silent from two blocks on");
}

/// A matrix row moves the part's controls as the knobs would: Velocity to Damping at -50% at full
/// velocity plays the crate with damping lowered by 0.5, Mod wheel to Bow at +100% with the wheel
/// at the top plays it with the bow at its ceiling, and Aftertouch to Strength at -50% with full
/// pressure lowers the strength by 0.5.
#[test]
fn matrix_rows_move_the_parts_controls() {
    let base = patch_of(0.2, 0.0, 0.5);
    let mut params = params_of(0, &base);
    params.extend([
        ("mod1_source", src::VELOCITY as f32),
        ("mod1_dest", D_DAMPING as f32),
        ("mod1_amount", -50.0),
        ("mod2_source", src::MOD_WHEEL as f32),
        ("mod2_dest", D_BOW as f32),
        ("mod2_amount", 100.0),
        ("mod3_source", src::AFTERTOUCH as f32),
        ("mod3_dest", D_STRENGTH as f32),
        ("mod3_amount", -50.0),
    ]);
    let mut r = instrument(32_000, &params);
    let blocks = 1000;
    // Full channel pressure, the wheel at the top, then the note, all before the first block.
    r.midi(0, [0xD0, 127, 0]);
    let got = render(&mut r, blocks * BLOCK, 128, &[(0, Ev::Cc(1, 127)), (0, Ev::On(60, 127))]);
    let stored = as_stored(&base);
    let patch = Patch { exciter_bow_level: CONTROL_MAX, resonator_damping: stored.resonator_damping - 0.5, ..stored };
    let mut part = ModalVoice::new();
    *part.patch_mut() = patch;
    let mut want = [Vec::new(), Vec::new()];
    for _ in 0..blocks {
        let performance = PerformanceState { gate: true, note: 60.0, modulation: 0.0, strength: 1.0 - 0.5 };
        let (mut m, mut a) = ([0.0f32; BLOCK], [0.0f32; BLOCK]);
        part.process(&performance, &[0.0; BLOCK], &[0.0; BLOCK], &mut m, &mut a);
        want[0].extend_from_slice(&a);
        want[1].extend_from_slice(&m);
    }
    assert!(same_bits(&got, &want), "the rows' sums reach the part");
}

/// A script with every kind of change, parameters at frames that start a host block in both
/// splits: the model (the FM pair too), the shaper switched with the gate up and off again, the
/// wheel, the bend, the pedal.
fn busy_script() -> Vec<(usize, Ev)> {
    vec![
        (0, Ev::On(60, 100)),
        (700, Ev::On(64, 90)),
        (701, Ev::On(67, 40)),
        (9000, Ev::Off(67)),
        (12_288, Ev::Param("model", 1.0)),
        (12_300, Ev::On(55, 127)),
        (20_000, Ev::Bend(12_000)),
        (24_576, Ev::Param("model", 3.0)),
        (24_600, Ev::On(72, 70)),
        (30_000, Ev::Cc(1, 90)),
        (36_864, Ev::Param("shaper", 1.0)),
        (40_000, Ev::Off(64)),
        (40_001, Ev::Off(60)),
        (40_002, Ev::Off(55)),
        (40_003, Ev::Cc(64, 127)),
        (40_004, Ev::Off(72)),
        (45_056, Ev::Param("model", 2.0)),
        (45_100, Ev::On(48, 127)),
        (52_000, Ev::Cc(64, 0)),
        (57_344, Ev::Param("shaper", 0.0)),
        (60_000, Ev::On(62, 127)),
        (70_000, Ev::Off(62)),
        (75_000, Ev::On(50, 127)),
    ]
}

/// The shaper's filter and envelopes, LFO 1 to Brightness, the wheel to the cutoff, LFO 2 to Pan,
/// the bow and the blow sounding.
const BUSY_PARAMS: [(&str, f32); 16] = [
    ("bow_level", 40.0),
    ("blow_level", 30.0),
    ("filter_mode", 0.0),
    ("cutoff", 1500.0),
    ("resonance", 40.0),
    ("filter_env_amount", 50.0),
    ("amp_release", 200.0),
    ("mod1_source", 1.0),
    ("mod1_dest", 3.0),
    ("mod1_amount", 30.0),
    ("mod2_source", 10.0),
    ("mod2_dest", 7.0),
    ("mod2_amount", 40.0),
    ("mod3_source", 2.0),
    ("mod3_dest", 14.0),
    ("mod3_amount", 60.0),
];

/// 64- and 512-frame host blocks render the same bits at 32, 44.1, 48 and 96 kHz through every kind
/// of change (the blocks are counted in output frames, and the edge's timing is exact).
#[test]
fn host_block_size_changes_no_bit_at_every_rate() {
    for sr in [32_000u32, 44_100, 48_000, 96_000] {
        let frames = sr as usize * 3;
        let mut a = instrument(sr, &BUSY_PARAMS);
        let mut b = instrument(sr, &BUSY_PARAMS);
        let ya = render(&mut a, frames, 64, &busy_script());
        let yb = render(&mut b, frames, 512, &busy_script());
        assert!(peak(&ya[0]) > 0.01 && peak(&ya[1]) > 0.01, "{sr} Hz: silent");
        assert!(same_bits(&ya, &yb), "{sr} Hz: 64- and 512-frame host blocks differ");
        assert!(ya.iter().flatten().all(|v| v.is_finite()));
    }
}

/// The greatest common divisor, for the impulse's placement.
fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// The edge from 32 kHz delays an impulse by exactly the latency the instrument reports, at every
/// session rate (an impulse on an engine frame that falls on a session sample); the table is the
/// one the help and ADR 0033 state.
#[test]
fn the_edge_delays_an_impulse_by_its_reported_latency() {
    let kernel = Kernel::new();
    let mut table = Vec::new();
    for sr in [22_050u32, 32_000, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000] {
        let (mut up, latency) = edge(&kernel, sr);
        let step = (ENGINE_RATE / gcd(ENGINE_RATE, sr)) as usize;
        let engine_at = step * 2000usize.div_ceil(step);
        let at = engine_at * sr as usize / ENGINE_RATE as usize;
        let mut pushed = 0usize;
        let y: Vec<f32> = (0..at + 2000)
            .map(|_| {
                while !up.ready() {
                    up.push(if pushed == engine_at { [0.5, -0.5] } else { [0.0; 2] });
                    pushed += 1;
                }
                up.pull(&kernel)[0]
            })
            .collect();
        let peak_at = (0..y.len()).max_by(|&i, &j| y[i].abs().total_cmp(&y[j].abs())).unwrap_or(0);
        assert_eq!(peak_at, at + latency, "{sr} Hz");
        assert!(y[peak_at] > 0.15, "{sr} Hz: peak {}", y[peak_at]);
        if sr == ENGINE_RATE {
            assert_eq!(y[peak_at].to_bits(), 0.5f32.to_bits(), "32 kHz is a copy");
        }
        println!("{sr} Hz: latency {latency} samples ({:.3} ms)", latency as f64 * 1000.0 / f64::from(sr));
        table.push((sr, latency));
        let r = instrument(sr, &[]);
        assert_eq!(r.latency(), latency, "{sr} Hz: the instrument reports its edge");
    }
    for (sr, latency) in STATED_LATENCY {
        assert!(table.contains(&(sr, latency)), "{sr} Hz: the stated latency {latency} is not the measured one ({table:?})");
    }
}

/// The latency table the help (`modal_voice.rate`) and the ADR 0033 note state, session samples.
const STATED_LATENCY: [(u32, usize); 8] =
    [(22_050, 35), (32_000, 0), (44_100, 49), (48_000, 53), (88_200, 97), (96_000, 105), (176_400, 193), (192_000, 210)];

/// The frequency of the strongest spectral peak within 1.5% of `around_hz` and its log magnitude
/// (a Hann-windowed DFT in 0.1 Hz steps, refined by a parabola), as the Resonator's pitch test
/// measures.
fn dft_peak(x: &[f64], rate: f64, around_hz: f64) -> (f64, f64) {
    let n = x.len();
    let w: Vec<f64> = x.iter().enumerate().map(|(i, v)| v * (0.5 - 0.5 * (TAU * i as f64 / n as f64).cos())).collect();
    let mag = |f: f64| {
        let (s, c) = (TAU * f / rate).sin_cos();
        let (mut re, mut im, mut cr, mut ci) = (0.0, 0.0, 1.0f64, 0.0f64);
        for (i, v) in w.iter().enumerate() {
            re += v * cr;
            im += v * ci;
            (cr, ci) = (cr * c - ci * s, cr * s + ci * c);
            if i % 4096 == 4095 {
                let m = (cr * cr + ci * ci).sqrt();
                (cr, ci) = (cr / m, ci / m);
            }
        }
        (re * re + im * im).sqrt().max(1e-30).ln()
    };
    let step = 0.1;
    let (lo, hi) = (around_hz / 1.015, around_hz * 1.015);
    let steps = ((hi - lo) / step) as usize;
    let mags: Vec<f64> = (0..=steps).map(|k| mag(lo + k as f64 * step)).collect();
    let k = (1..steps).max_by(|a, b| mags[*a].total_cmp(&mags[*b])).unwrap_or(1);
    let (a, m, c) = (mags[k - 1], mags[k], mags[k + 1]);
    let d = 0.5 * (a - c) / (a - 2.0 * m + c);
    (lo + (k as f64 + d) * step, m - 0.25 * (a - c) * d)
}

/// The pitch near `expected_hz`, refusing an octave-down alias (the spectrum at half the pitch
/// must be 12 dB under the peak) and an octave-up one (likewise at twice the pitch).
fn pitch_hz(x: &[f64], rate: f64, expected_hz: f64, label: &str) -> f64 {
    let (hz, peak) = dft_peak(x, rate, expected_hz);
    for alias in [0.5, 2.0] {
        let (_, m) = dft_peak(x, rate, hz * alias);
        let below_db = 20.0 * (peak - m) / std::f64::consts::LN_10;
        assert!(below_db > 12.0, "{label}: the spectrum at {alias}x the pitch is only {below_db:.1} dB under the peak");
    }
    hz
}

/// A struck and held A4 (MIDI 69) sounds at 440 Hz within 1 cent at 32, 44.1, 48, 88.2 and 96 kHz
/// (E63): the part runs at its own 32 kHz behind the edge whatever the session rate. The modal
/// model with geometry 0.25 (harmonic modes) at full damping, brightness 0 (the upper modes die
/// first, so the fundamental leads the spectrum by more than 12 dB), the strike alone with a dark
/// mallet, the reverb dry (space 0), one second from 0.2 s after the note, on the right output.
#[test]
fn a_held_a4_plays_440_hz_at_every_session_rate() {
    for sr in [32_000u32, 44_100, 48_000, 88_200, 96_000] {
        let rate = f64::from(sr);
        let params = [
            ("geometry", 25.0),
            ("damping", 100.0),
            ("space", 0.0),
            ("strike_level", 60.0),
            ("level", 0.0),
            ("brightness", 0.0),
            ("strike_timbre", 20.0),
        ];
        let mut r = instrument(sr, &params);
        let y = render(&mut r, sr as usize * 13 / 10, 256, &[(0, Ev::On(69, 127))]);
        let from = sr as usize / 5;
        let window: Vec<f64> = y[1][from..from + sr as usize].iter().map(|&v| f64::from(v)).collect();
        assert!(window.iter().any(|v| v.abs() > 0.01), "{sr} Hz: silent");
        let label = format!("{sr} Hz");
        let hz = pitch_hz(&window, rate, 440.0, &label);
        let cents = 1200.0 * (hz / 440.0).log2();
        println!("{label}: {hz:.4} Hz ({cents:+.4} cents)");
        assert!(cents.abs() < 1.0, "{label}: {hz} Hz ({cents:+.3} cents)");
    }
}

/// The measurement refuses an octave error: a tone at the expected pitch's half (with nothing at
/// the expected pitch) and one at its double both fail.
#[test]
fn the_pitch_measurement_refuses_octave_aliases() {
    let rate = 48_000.0;
    for f in [440.0, 1760.0] {
        let x: Vec<f64> = (0..24_000).map(|i| (TAU * f * i as f64 / rate).sin() + 0.2 * (TAU * 880.0 * i as f64 / rate).sin()).collect();
        let refused = std::panic::catch_unwind(|| pitch_hz(&x, rate, 880.0, "alias"));
        assert!(refused.is_err(), "{f} Hz with a weak 880 Hz partial passed as 880 Hz");
    }
}

/// The largest step between neighboring samples in `x[range]`.
fn largest_step(x: &[f32], range: std::ops::Range<usize>) -> f32 {
    x[range].windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()))
}

/// The shaper switched on while the gate is up (#262) crossfades into it over one 12-frame shaper
/// block, then plays exactly what a render shaped from the start plays (the filter and the
/// envelopes run either way); switched off, it crossfades back and plays the part's bits. The
/// crossfade steps by no more than the two paths do plus a twelfth of the gap between them.
#[test]
fn switching_the_shaper_while_the_gate_is_up_crossfades_at_once() {
    let shaped = [("shaper", 1.0), ("cutoff", 800.0), ("resonance", 30.0), ("amp_sustain", 70.0), ("level", 0.0), ("bow_level", 50.0)];
    let mut dry = shaped;
    dry[0] = ("shaper", 0.0);
    let frames = 32_000;
    let notes = [(0, Ev::On(57, 127))];
    let always = render(&mut instrument(32_000, &shaped), frames, 256, &notes);
    let never = render(&mut instrument(32_000, &dry), frames, 256, &notes);
    // On at frame 12 288: a part block's and a shaper block's first frame.
    let at = 12_288;
    let mut on = notes.to_vec();
    on.push((at, Ev::Param("shaper", 1.0)));
    let switched = render(&mut instrument(32_000, &dry), frames, 256, &on);
    for c in 0..2 {
        assert!(same_from(&switched[c], &never[c], 0, at), "channel {c}: before the switch, the part");
        assert!(same_from(&switched[c], &always[c], at + SHAPER_BLOCK, frames), "channel {c}: after the fade, shaped");
        let step = largest_step(&switched[c], at - 1..at + SHAPER_BLOCK + 1);
        let own = largest_step(&never[c], at - 1..at + SHAPER_BLOCK + 1).max(largest_step(&always[c], at - 1..at + SHAPER_BLOCK + 1));
        let gap = never[c][at..at + SHAPER_BLOCK].iter().zip(&always[c][at..at + SHAPER_BLOCK]).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        println!("on, channel {c}: largest step {step:.5} (the paths' own {own:.5}, the gap between them {gap:.5})");
        assert!(gap > 0.001, "channel {c}: the paths differ");
        assert!(step <= own + gap / SHAPER_BLOCK as f32 + 1e-6, "channel {c}: the switch steps by {step}");
    }
    let off = 24_576;
    on.push((off, Ev::Param("shaper", 0.0)));
    let back = render(&mut instrument(32_000, &dry), frames, 256, &on);
    for c in 0..2 {
        assert!(same_from(&back[c], &never[c], off + SHAPER_BLOCK, frames), "channel {c}: after the fade back, the part");
    }
}

/// With the gate down the switch waits for the next note (the ringing tail is not faded into the
/// filter), and that note takes it.
#[test]
fn with_the_gate_down_the_shaper_switch_waits_for_the_next_note() {
    let dry = [("shaper", 0.0), ("cutoff", 600.0), ("level", 0.0), ("damping", 90.0)];
    let frames = 32_000;
    let base = [(0, Ev::On(57, 127)), (1600, Ev::Off(57)), (16_000, Ev::On(64, 127))];
    let never = render(&mut instrument(32_000, &dry), frames, 256, &base);
    let mut switched = base.to_vec();
    switched.push((8192, Ev::Param("shaper", 1.0)));
    let got = render(&mut instrument(32_000, &dry), frames, 256, &switched);
    let note = 16_000;
    assert!(same_from(&got[1], &never[1], 0, note), "the tail plays on unshaped");
    assert!(!same_from(&got[1], &never[1], note + 2 * SHAPER_BLOCK, frames), "the next note is shaped");
}

/// A reset instrument renders what a new one renders, through the edge (44.1 kHz) and at 32 kHz,
/// after a busy run.
#[test]
fn a_reset_instrument_renders_what_a_new_one_renders() {
    for sr in [44_100u32, 32_000] {
        let mut used = instrument(sr, &BUSY_PARAMS);
        let _ = render(&mut used, sr as usize * 2, 300, &busy_script());
        used.reset();
        let mut fresh = instrument(sr, &BUSY_PARAMS);
        for (id, v) in [("model", 0.0), ("shaper", 0.0)] {
            assert!(used.set_param(id, v) && fresh.set_param(id, v));
        }
        let script = [(0, Ev::On(60, 100)), (5000, Ev::On(67, 127)), (9000, Ev::Off(67))];
        let a = render(&mut used, 24_000, 256, &script);
        let b = render(&mut fresh, 24_000, 256, &script);
        assert!(peak(&a[0]) > 0.01, "{sr} Hz: silent");
        assert!(same_bits(&a, &b), "{sr} Hz: reset is not new");
    }
}

/// A mono strip hears the mean of the two sides; hostile values keep the output finite.
#[test]
fn a_mono_strip_and_hostile_values() {
    let mut mono = ModalVoiceInstrument::new();
    mono.prepare(48_000.0, 256, 1);
    mono.note_on(0, 60, 127);
    let mut stereo = instrument(48_000, &[]);
    stereo.note_on(0, 60, 127);
    let (mut io1, mut io2) = (vec![vec![0.0f32; 256]], vec![vec![0.0f32; 256]; 2]);
    for _ in 0..20 {
        mono.process(&mut io1, 256);
        stereo.process(&mut io2, 256);
        for (m, (l, r)) in io1[0].iter().zip(io2[0].iter().zip(&io2[1])) {
            assert_eq!(m.to_bits(), ((l + r) * 0.5).to_bits());
        }
    }
    let mut h = instrument(48_000, &[("tune", 24.0), ("bend_range", 24.0), ("geometry", 100.0), ("damping", 100.0), ("space", 200.0)]);
    for (id, v) in [("brightness", f32::NAN), ("position", f32::INFINITY), ("tune", -1e9), ("bow_level", 1e9)] {
        h.set_param(id, v);
    }
    let script = [(0, Ev::On(127, 127)), (10, Ev::On(0, 127)), (20, Ev::Bend(16_383)), (5000, Ev::Bend(0))];
    let y = render(&mut h, 48_000, 512, &script);
    assert!(y.iter().flatten().all(|v| v.is_finite()), "finite");
}

/// A note of 0.4 s on `key` at velocity `vel`, then `tail` frames; returns the stereo render at
/// 32 kHz in 256-frame blocks.
fn note_then_silence(params: &[(&str, f32)], vel: u8, extra: Option<(usize, Ev)>, frames: usize) -> [Vec<f32>; 2] {
    let mut script = vec![(0, Ev::On(57, vel)), (12_800, Ev::Off(57))];
    script.extend(extra);
    render(&mut instrument(32_000, params), frames, 256, &script)
}

/// With nothing held no model sounds: not at creation, not after a reset (the FM pair's voice
/// opens from the strength input as well as its envelope; the module drones, the instrument closes
/// it).
#[test]
fn every_model_is_silent_before_any_note_and_after_a_reset() {
    for model in 0..4 {
        let params = params_of(model, &patch_of(0.6, 0.3, 0.8));
        let mut r = instrument(32_000, &params);
        let fresh = render(&mut r, 32_000, 256, &[]);
        assert_eq!(peak(&fresh[0]).max(peak(&fresh[1])), 0.0, "model {model}: sounds at creation");
        render(&mut r, 8000, 256, &[(0, Ev::On(60, 127))]);
        r.reset();
        let again = render(&mut r, 32_000, 256, &[]);
        assert_eq!(peak(&again[0]).max(peak(&again[1])), 0.0, "model {model}: sounds after reset");
    }
}

/// The FM pair is silent (decayed below -120 dB, the ring's own tail) after the key is released, at every velocity and at Velocity 0%, and
/// holds its sound while the key is down; CC 123 and CC 120 leave it silent too.
#[test]
fn the_fm_pair_is_silent_once_no_key_is_held() {
    let base = params_of(3, &patch_of(0.6, 0.3, 0.8));
    for (vel, amount) in [(40u8, 100.0), (64, 100.0), (127, 100.0), (100, 0.0), (127, 0.0)] {
        let mut params = base.clone();
        params.push(("velocity", amount));
        let out = note_then_silence(&params, vel, None, 32_000 * 10);
        let held = peak(&out[0][3000..12_800]).max(peak(&out[1][3000..12_800]));
        let late = peak(&out[0][32_000 * 8..]).max(peak(&out[1][32_000 * 8..]));
        let at = |s: usize| peak(&out[0][32_000 * s..32_000 * (s + 1)]);
        println!(
            "fm pair velocity {vel} amount {amount}: held {held} late {late}; per second after the key {} {} {} {} {} {} {} {}",
            at(0),
            at(1),
            at(2),
            at(3),
            at(4),
            at(5),
            at(6),
            at(7)
        );
        assert!(held > 0.01, "velocity {vel} amount {amount}: the held FM pair sounds");
        assert!(late < 1e-6, "velocity {vel} amount {amount}: sounds after the release ({late})");
    }
    for cc in [123u8, 120] {
        let out = render(&mut instrument(32_000, &base), 32_000 * 10, 256, &[(0, Ev::On(57, 127)), (12_800, Ev::Cc(cc, 0))]);
        let held = peak(&out[0][3000..12_800]);
        let late = peak(&out[0][32_000 * 8..]).max(peak(&out[1][32_000 * 8..]));
        println!("fm pair cc {cc}: held {held} late {late}");
        assert!(held > 0.01);
        assert!(late < 1e-6, "CC {cc}: the FM pair sounds with no key held ({late})");
    }
}

/// Closing the FM pair is a fade: releasing a full-velocity note moves no sample further than
/// the held voice's own largest step times four.
#[test]
fn closing_the_fm_pair_does_not_click() {
    let params = params_of(3, &patch_of(0.6, 0.3, 0.8));
    let out = note_then_silence(&params, 127, None, 32_000 * 3);
    let held = largest_step(&out[0], 3000..12_800);
    let released = largest_step(&out[0], 12_800..32_000 * 3);
    println!("fm pair largest step: held {held} release {released}");
    assert!(released <= held * 4.0 + 1e-4, "the release steps {released} against {held} held");
}

/// CC 120 silences every model, on both channels, at once.
#[test]
fn all_sound_off_silences_every_model() {
    for model in 0..4 {
        let params = params_of(model, &patch_of(0.6, 0.3, 0.8));
        let out = render(&mut instrument(32_000, &params), 16_000, 256, &[(0, Ev::On(57, 100)), (4800, Ev::Cc(120, 0))]);
        assert!(peak(&out[0][..4800]).max(peak(&out[1][..4800])) > 0.001, "model {model}: sounds before the CC");
        assert_eq!(peak(&out[0][4800 + 32..]), 0.0, "model {model}: left not silent");
        assert_eq!(peak(&out[1][4800 + 32..]), 0.0, "model {model}: right not silent");
    }
}

/// A note shorter than one engine block (a note-on and its note-off in the same 16 frames) still
/// plays: the gate rises for one block.
#[test]
fn a_note_shorter_than_one_engine_block_sounds() {
    let params = params_of(0, &patch_of(0.0, 0.0, 0.9));
    let out = render(&mut instrument(32_000, &params), 8000, 256, &[(100, Ev::On(60, 127)), (104, Ev::Off(60))]);
    let p = peak(&out[0]).max(peak(&out[1]));
    println!("short note peak {p}");
    assert!(p > 0.001, "the short note was dropped");
}
