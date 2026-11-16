//! #525 and #601: the device-rate converter's output is a function of the rendered stream alone,
//! and it is the band-limited reader (ADR 0056 decision 8) at an exact position. #525 found it
//! holding the last frame of every rendered block instead of reading into the next block; #601
//! replaced its linear interpolation with the reader.
//!
//! Three layers: the converter's primitives (`Phase`, `History`) driven over a synthetic stream cut
//! into arbitrary render blocks, compared bit for bit with the reader over the whole stream; the
//! live player through its fake device (no real device); and the converter's measured quality
//! (passband and alias figures, deterministic tones) against the reader's acceptance bounds.
use super::super::phase::{History, Phase};
use super::*;
use crate::band_limited;
use transit_model::{ChannelFormat, Clip, SourceAudio, SourceId, TrackKind};
use transit_resample::Mono;

const RATES: [u32; 5] = [44_100, 48_000, 88_200, 96_000, 192_000];
/// Ratios no common rate pair has (session, device).
const ODD: [(u32, u32); 2] = [(44_100, 47_999), (47_999, 44_100)];
const PARTITIONS: [&[usize]; 6] = [&[1], &[7], &[64], &[480], &[4096], &[1, 7, 4096, 3, 64, 480, 2, 511, 13]];

fn pairs() -> Vec<(u32, u32)> {
    RATES.iter().flat_map(|&session| RATES.iter().map(move |&device| (session, device))).chain(ODD).collect()
}

/// Output frames read straight from the whole input with the reader: output frame `k` at the
/// position `units / d` (`d` = device rate x 1000), `units` advancing by `session x speed` per
/// output frame, the speed changing to `speeds[i].1` from output frame `speeds[i].0` on (the
/// position carries on from where it is: nothing jumps).
fn reference_with(input: &[f32], session: u32, device: u32, speeds: &[(usize, u32)], frames: usize) -> Vec<f32> {
    let d = i64::from(device) * 1000;
    let table = band_limited::table();
    let mut units = 0i64;
    let mut out = Vec::with_capacity(frames);
    let mut one = [0.0f32];
    for k in 0..frames {
        let speed = speeds.iter().rev().find(|(from, _)| *from <= k).map_or(1000, |(_, s)| *s);
        let step = i64::from(session) * i64::from(speed);
        let q = step as f64 / d as f64;
        band_limited::frame(table, &Mono(input), band_limited::position(units, d), q, &mut one);
        out.push(one[0]);
        units += step;
    }
    out
}

fn reference(input: &[f32], session: u32, device: u32, speed: u32, frames: usize) -> Vec<f32> {
    reference_with(input, session, device, &[(0, speed)], frames)
}

/// The converter's own primitives, in the order `fill_inner` uses them, over `input` rendered in
/// blocks of the sizes in `partition` (repeated), with the speed schedule of [`reference_with`].
fn convert_with(input: &[f32], session: u32, device: u32, speeds: &[(usize, u32)], partition: &[usize], frames: usize) -> Vec<f32> {
    let mut phase = Phase::new(device).unwrap();
    let mut history = History::new();
    let mut block = vec![vec![0.0f32; 4096]];
    let (mut pending, mut cursor, mut sizes) = (0, 0, partition.iter().cycle());
    let mut out = Vec::with_capacity(frames);
    let mut one = [0.0f32];
    // The largest rate the schedule reaches bounds the tail a block keeps, as the player's 4x does.
    let top = speeds.iter().map(|(_, s)| *s).max().unwrap_or(1000);
    let q_max = f64::from(session) * f64::from(top) / (f64::from(device) * 1000.0);
    for k in 0..frames {
        let speed = speeds.iter().rev().find(|(from, _)| *from <= k).map_or(1000, |(_, s)| *s);
        let step = u64::from(session) * u64::from(speed);
        while phase.needs(pending, step) {
            let tail = phase.reachable_tail(pending, step, q_max);
            history.keep(&block, pending, tail);
            phase.remove(pending).unwrap();
            pending = *sizes.next().unwrap();
            for (slot, value) in block[0].iter_mut().zip(input.iter().skip(cursor).chain(std::iter::repeat(&0.0))).take(pending) {
                *slot = *value;
            }
            cursor += pending;
        }
        let (x, q) = phase.position(step);
        history.frame(&block, pending, x, q, &mut one);
        out.push(one[0]);
        phase.advance(step).unwrap();
    }
    assert_eq!(history.misses(), 0, "{session} -> {device}: a read outside the kept and the pending frames");
    out
}

fn convert(input: &[f32], session: u32, device: u32, speed: u32, partition: &[usize], frames: usize) -> Vec<f32> {
    convert_with(input, session, device, &[(0, speed)], partition, frames)
}

/// Deterministic full-scale noise, IEEE-exact (24-bit integers scaled by a power of two).
fn noise(frames: usize, seed: u32) -> Vec<f32> {
    let mut state = seed;
    (0..frames)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f32 / 8_388_608.0 - 1.0
        })
        .collect()
}

/// Input frames `frames` output frames read, with the reader's reach past the last one.
fn input_frames(session: u32, device: u32, speed: u32, frames: usize) -> usize {
    (frames as u128 * u128::from(session) * u128::from(speed) / (u128::from(device) * 1000)) as usize + 16 * 32 + 2
}

fn bits(samples: &[f32]) -> Vec<u32> {
    samples.iter().map(|s| s.to_bits()).collect()
}

#[test]
fn the_converter_output_is_independent_of_the_render_partition_at_every_rate_pair() {
    const FRAMES: usize = 6000;
    for (session, device) in pairs() {
        for speed in [1000, 500] {
            let input = noise(input_frames(session, device, speed, FRAMES), session ^ device);
            let expected = bits(&reference(&input, session, device, speed, FRAMES));
            for partition in PARTITIONS {
                let got = bits(&convert(&input, session, device, speed, partition, FRAMES));
                let first = got.iter().zip(&expected).position(|(g, e)| g != e);
                assert_eq!(first, None, "{session} -> {device} at speed {speed}, blocks {partition:?}: first differing output frame");
            }
        }
    }
}

#[test]
fn equal_rates_pass_every_bit_through_whatever_the_partition() {
    let mut input = noise(5000, 7);
    input[..6].copy_from_slice(&[-0.0, 0.0, f32::from_bits(1), -f32::MAX, f32::MAX, f32::MIN_POSITIVE]);
    for rate in RATES {
        for partition in PARTITIONS {
            assert_eq!(bits(&convert(&input, rate, rate, 1000, partition, 4998)), bits(&input[..4998]), "{rate} Hz, blocks {partition:?}");
        }
    }
}

/// B1 of the #601 review: equal rates at speed 1 cost what they did before the reader. Every output
/// frame takes the integer fast path (no reader call), and each block keeps only the tail a read
/// could still reach after a change to the fastest speed: `halo(4)` = 65 frames of each channel at
/// 48 kHz on 48 kHz, against the one frame of 80 channel slots the linear converter kept (130 sample
/// writes for a stereo block against 80). The player through its fake device makes no reader call
/// at all at equal rates (`play` checks it).
#[test]
fn equal_rates_take_the_fast_path_and_keep_only_the_reachable_tail() {
    use transit_resample::halo;
    let input = noise(512 * 60, 3);
    let mut phase = Phase::new(48_000).unwrap();
    let mut history = History::new();
    let mut block = vec![vec![0.0f32; 512]; 2];
    let (step, q_max) = (48_000u64 * 1000, 4.0);
    let (mut pending, mut cursor, mut blocks) = (0usize, 0usize, 0u64);
    for k in 0..512 * 50 {
        while phase.needs(pending, step) {
            let tail = phase.reachable_tail(pending, step, q_max);
            history.keep(&block, pending, tail);
            phase.remove(pending).unwrap();
            pending = 512;
            for channel in &mut block {
                channel.copy_from_slice(&input[cursor..cursor + 512]);
            }
            cursor += 512;
            blocks += 1;
        }
        let i = phase.whole(step).expect("equal rates at speed 1 sit on whole frames");
        assert_eq!(block[0][i].to_bits(), input[k].to_bits());
        phase.advance(step).unwrap();
    }
    assert_eq!(history.reads(), 0);
    // The first block kept nothing (nothing was pending); every later one its 65-frame tail.
    assert_eq!(history.written(), (blocks - 1) * halo(4.0) as u64 * 2, "{blocks} blocks");
    let _ = play(48_000, 48_000, 1000, &[512, 7, 1], 9_000);
}

/// A speed change moves the rate, never the position: the converter carries on from the exact
/// position it reached, so its output is the reader over the whole stream at that piecewise
/// position, whatever the partition, through changes up and down and back to unit rate.
#[test]
fn a_speed_change_continues_from_the_same_position() {
    const FRAMES: usize = 6000;
    let speeds: [(usize, u32); 4] = [(0, 1000), (1_234, 1_250), (2_500, 500), (4_000, 1000)];
    for (session, device) in [(48_000, 48_000), (44_100, 48_000), (96_000, 44_100)] {
        let input = noise(input_frames(session, device, 1_250, FRAMES), 11);
        let expected = bits(&reference_with(&input, session, device, &speeds, FRAMES));
        for partition in PARTITIONS {
            let got = bits(&convert_with(&input, session, device, &speeds, partition, FRAMES));
            let first = got.iter().zip(&expected).position(|(g, e)| g != e);
            assert_eq!(first, None, "{session} -> {device}, blocks {partition:?}: first differing output frame");
        }
    }
}

/// The reader is linear phase, so the converter adds no delay: an impulse at render frame `S`
/// peaks at the output frame whose position is nearest `S`, the frame the timeline maps `S` to
/// (the record start point and the MIDI stamps use that map), as it did with the linear converter.
/// What the reader adds is render-ahead: the block holding `S` is rendered up to the reader's reach
/// before that output frame.
#[test]
fn an_impulse_peaks_at_the_output_frame_the_timeline_maps_it_to() {
    const AT: usize = 3_001;
    for (session, device) in [(44_100, 48_000), (48_000, 44_100), (48_000, 96_000), (96_000, 48_000), (44_100, 47_999)] {
        let mut input = vec![0.0f32; input_frames(session, device, 1000, 8_000)];
        input[AT] = 0.5;
        let out = convert(&input, session, device, 1000, &[64], 8_000);
        let peak = (0..out.len()).max_by(|a, b| out[*a].abs().total_cmp(&out[*b].abs())).unwrap();
        let q = f64::from(session) / f64::from(device);
        let nearest = (AT as f64 / q).round() as usize;
        assert_eq!(peak, nearest, "{session} -> {device}: the impulse peaks at output frame {peak}, the map puts it at {nearest}");
    }
}

/// One tone through the converter: (level at the bin `heard` in dB from the tone's amplitude, that
/// bin over everything else in dB). `frames` output frames, after a warm-up of as many,
/// hold a whole number of the tone's cycles and of the converter's phase pattern, so every
/// component sits on a DFT bin (no window, no leakage).
fn tone(session: u32, device: u32, hz: u32, heard: u32, frames: usize) -> (f64, f64) {
    use transit_math::f64 as m;
    const AMPLITUDE: f64 = 0.5;
    let tau = 2.0 * core::f64::consts::PI;
    let input: Vec<f32> = (0..input_frames(session, device, 1000, 2 * frames))
        .map(|n| (AMPLITUDE * m::sin(tau * f64::from(hz) * n as f64 / f64::from(session))) as f32)
        .collect();
    let out = convert(&input, session, device, 1000, &[1, 7, 4096, 3, 64, 480], 2 * frames);
    let (mut re, mut im, mut total) = (0.0, 0.0, 0.0);
    for (n, &y) in out.iter().enumerate().skip(frames) {
        let y = f64::from(y);
        // The bin's angle reduced exactly (integer arithmetic) before the sine.
        let turn = (u64::from(heard) * n as u64 % u64::from(device)) as f64 / f64::from(device);
        let (s, c) = m::sin_cos(tau * turn);
        re += y * c;
        im -= y * s;
        total += y * y;
    }
    let n = frames as f64;
    let at = 2.0 * (re * re + im * im) / (n * n);
    let rest = (total / n - at).max(1e-30);
    let gain = 10.0 * m::log10(at / (AMPLITUDE * AMPLITUDE / 2.0));
    (gain, 10.0 * m::log10(at / rest))
}

/// The passband error bound of ADR 0056 decision 8.4 at rate `q`, as an amplitude ratio.
fn passband_bound(q: f64) -> f64 {
    10f64.powf(if q < 1.5 { -73.0 } else { -78.0 } / 20.0)
}

/// The converter at every common rate pair both ways (44.1, 48, 88.2 and 96 kHz), against the
/// reader's acceptance bounds (ADR 0056 decision 8.4). The bands are those of the reader with the
/// source at the session rate and the output at the device rate: the passband holds tones up to
/// 0.42 of the lower of the two rates (18.5 kHz when either is 44.1 kHz, 20.2 kHz at 48 kHz); the
/// alias band holds source tones that would play at 0.58 of the device rate or above.
///
/// A passband error of at most `e` of the tone's amplitude bounds the tone's measured gain within
/// `20·log10(1 ± 2e)` dB (the error's projection on the bin) and the tone over everything else at
/// `−20·log10(e) − 3.01` dB or more (the error's power at most `e²` of the squared amplitude, the
/// tone's half). An alias tone's level at the bin it folds to is at most the alias bound. The
/// figures are printed; the help and the known issue quote them.
#[test]
fn the_converter_meets_the_readers_bounds_at_every_common_rate_pair() {
    const COMMON: [u32; 4] = [44_100, 48_000, 88_200, 96_000];
    let mut failures = Vec::new();
    for session in COMMON {
        for device in COMMON.into_iter().filter(|d| *d != session) {
            let q = f64::from(session) / f64::from(device);
            let frames = device as usize / 10;
            let low = session.min(device);
            let edge = (u64::from(low) * 42 / 100 / 10 * 10) as u32;
            let e = passband_bound(q);
            let (gain_bound, rest_bound) = (20.0 * (1.0 + 2.0 * e).log10(), -20.0 * e.log10() - 3.0103);
            for hz in [1_000, 10_000, edge] {
                let (gain, rest) = tone(session, device, hz, hz, frames);
                eprintln!("converter passband: {session} -> {device} (q {q:.5}), {hz} Hz: gain {gain:+.5} dB, tone over the rest {rest:.1} dB");
                if gain.abs() > gain_bound || rest < rest_bound {
                    failures.push(format!(
                        "{session} -> {device} {hz} Hz: gain {gain:+.5} dB (bound ±{gain_bound:.5}), rest {rest:.1} dB (bound {rest_bound:.1})"
                    ));
                }
            }
            // Source tones below the session's Nyquist that would play at 0.58 of the device rate
            // or above: 30 kHz at 88.2 and 96 kHz into 44.1 and 48 kHz.
            let alias_from = f64::from(device) * 0.58;
            if q > 1.0 && 30_000.0 >= alias_from && 30_000 < session / 2 {
                let heard = device - 30_000;
                let (level, _) = tone(session, device, 30_000, heard, frames);
                let bound = if q < 1.5 { -73.5 } else { -78.0 };
                eprintln!("converter alias: {session} -> {device} (q {q:.5}), 30000 Hz heard at {heard} Hz: {level:.1} dB (bound {bound})");
                if level > bound {
                    failures.push(format!("{session} -> {device} 30000 Hz alias at {heard} Hz: {level:.1} dB (bound {bound})"));
                }
            }
        }
    }
    // Transition-band observations (between 0.42 and 0.58 of the rate, decision 8.4's stated
    // limits), printed for the record, not bounded: the old known issue's cases.
    for (session, device, hz, heard) in [(44_100, 48_000, 20_000, 20_000), (48_000, 44_100, 20_000, 20_000), (48_000, 44_100, 23_000, 21_100)] {
        let (level, rest) = tone(session, device, hz, heard, device as usize / 10);
        eprintln!("converter transition band: {session} -> {device}, {hz} Hz heard at {heard} Hz: {level:+.2} dB, tone over the rest {rest:.1} dB");
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// The live player, through its fake device.

/// The ramp's step: a power of two, so `n * STEP` is exact.
const STEP: f32 = 1.0 / 131_072.0;

fn ramp_session(rate: u32, frames: usize) -> (Arc<Session>, [Vec<f32>; 2]) {
    let mut s = Session { sample_rate: transit_time::SampleRate::new(rate).unwrap(), ..Session::default() };
    s.edit.click = false;
    let track = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    let mut ramp: Vec<f32> = (0..frames).map(|n| n as f32 * STEP).collect();
    // The right channel is a sawtooth pattern with a jump every 31 frames (IEEE-exact values).
    let mut pattern: Vec<f32> = (0..frames).map(|n| (n % 31) as f32 / 64.0 - 0.25).collect();
    // F4 of the #601 review: a negative zero and a subnormal. The mix sums the negative zero to
    // +0.0 and sanitizes non-finite samples before the converter, so no sample value can tell the
    // direct frame from the reader through the player; there the fast path is pinned by `play`'s
    // count of reader calls (none at equal rates and speed 1). The input converter and the
    // primitives pin the negative zero itself.
    ramp[5] = -0.0;
    pattern[6] = f32::from_bits(1);
    s.pool.insert(
        SourceId(1),
        Arc::new(SourceAudio::new(transit_audio_io::AudioBuffer { sample_rate: rate, channels: vec![ramp.clone(), pattern.clone()] })),
    );
    let clip = s.new_clip_id();
    s.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(clip, "ramp", SourceId(1), 0, 0, frames as i64));
    (Arc::new(s), [ramp, pattern])
}

/// Play the ramp session from 0 at `speed` and collect `frames` stereo output frames in callbacks
/// of the sizes in `callbacks` (repeated).
fn play(session: u32, device: u32, speed: u32, callbacks: &[usize], frames: usize) -> (Vec<f32>, Vec<f32>, [Vec<f32>; 2]) {
    let (doc, input) = ramp_session(session, input_frames(session, device, speed, frames) + 1024);
    let shared = Arc::new(Shared {
        position: AtomicI64::new(0),
        output_frame: std::sync::atomic::AtomicU64::new(0),
        playing: AtomicBool::new(false),
        speed: AtomicU32::new(speed),
        editors_open: AtomicBool::new(false),
        monitor_armed: AtomicBool::new(true),
        metro: crate::Metro::default(),
        monitor: Default::default(),
    });
    let target = RenderTarget { device: DeviceGeneration(1), sample_rate: session, max_block: 512, out_channels: 0, in_channels: 0 };
    let key = ClockKey { generation: 1, domain: 1, direction: Direction::Output };
    let (mut control, mut audio) = pair(target, PlanLimits::default(), key, device, Arc::clone(&shared)).unwrap();
    control.request(doc, None).unwrap();
    control.service(&mut make);
    control.send_action(Command::Play { from: 0, end: None, looped: None }, true).unwrap();
    let (mut left, mut right) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
    let mut sizes = callbacks.iter().cycle();
    while left.len() < frames {
        let size = (*sizes.next().unwrap()).min(frames - left.len());
        let mut out = vec![0.0; size * 2];
        let ((), heap) = measure(|| fill(&mut audio, &mut out));
        assert!(heap.is_zero(), "{heap:?}");
        for frame in out.as_chunks::<2>().0 {
            left.push(frame[0]);
            right.push(frame[1]);
        }
    }
    assert_eq!(audio.history.misses(), 0, "{session} -> {device}: a read outside the kept and the pending frames");
    if session == device && speed == 1000 {
        assert_eq!(audio.history.reads(), 0, "{session} Hz at speed 1: every frame takes the fast path");
    }
    control.retire(audio.plans, audio.actions);
    (left, right, input)
}

/// The issue's case through the live player: every output frame is the reader over the rendered
/// stream at the exact position, bit for bit; and the ramp comes out at its closed form `k *
/// session * speed / device * STEP` within the reader's measured response to a constant, relative
/// to the value (decision 8.4: a filtered ramp is not exact). That limit is relative and governs
/// from frame 4,096 on; below it the kernel's first moment at each fraction (an absolute error of a
/// small fraction of one STEP) is not negligible against the value, and near frame 0 the taps
/// before the stream read silence.
#[test]
fn a_ramp_through_the_player_is_the_reader_at_every_output_frame() {
    const FRAMES: usize = 51_200;
    for (session, device, speed) in [(44_100, 48_000, 1000), (48_000, 44_100, 1000), (48_000, 96_000, 1000), (48_000, 48_000, 500)] {
        let (left, right, input) = play(session, device, speed, &[512], FRAMES);
        let q = f64::from(session) * f64::from(speed) / (f64::from(device) * 1000.0);
        let limit = if q <= 1.0 {
            4.2e-5
        } else if q < 1.5 {
            5.2e-5
        } else {
            3.7e-5
        };
        let mut worst = 0.0f64;
        for (k, &got) in left.iter().enumerate() {
            let position = k as f64 * q;
            if position < 4_096.0 {
                continue;
            }
            let expected = position * f64::from(STEP);
            worst = worst.max((f64::from(got) - expected).abs() / expected);
        }
        eprintln!("ramp through the player, {session} -> {device} at speed {speed}: worst relative error {worst:.3e} (limit {limit:.1e})");
        assert!(worst <= limit, "{session} -> {device} at speed {speed}: relative error {worst:.3e} over {limit:.1e}");
        assert_eq!(bits(&left), bits(&reference(&input[0], session, device, speed, FRAMES)), "{session} -> {device} left");
        assert_eq!(bits(&right), bits(&reference(&input[1], session, device, speed, FRAMES)), "{session} -> {device} right");
    }
}

#[test]
fn equal_rates_through_the_player_are_bit_exact_pass_through() {
    for rate in [44_100, 48_000, 96_000] {
        let (left, right, input) = play(rate, rate, 1000, &[512, 7, 1], 9000);
        // What reaches the device: the mix sums a negative zero to +0.0.
        let device = |x: &[f32]| x.iter().map(|v| if *v == 0.0 { 0.0 } else { *v }).collect::<Vec<f32>>();
        assert_eq!(bits(&left), bits(&device(&input[0][..9000])), "{rate} Hz left");
        assert_eq!(bits(&right), bits(&device(&input[1][..9000])), "{rate} Hz right");
    }
}

#[test]
fn the_player_output_is_independent_of_the_callback_partition() {
    const FRAMES: usize = 9600;
    for (session, device) in [(44_100, 48_000), (48_000, 44_100), (192_000, 44_100), (44_100, 192_000), (44_100, 47_999)] {
        let mut first: Option<(Vec<u32>, Vec<u32>)> = None;
        for callbacks in PARTITIONS {
            let (left, right, input) = play(session, device, 1000, callbacks, FRAMES);
            let got = (bits(&left), bits(&right));
            assert_eq!(got.0, bits(&reference(&input[0], session, device, 1000, FRAMES)), "{session} -> {device}, callbacks {callbacks:?}");
            match &first {
                None => first = Some(got),
                Some(first) => assert!(*first == got, "{session} -> {device}: callbacks {callbacks:?} differ from {:?}", PARTITIONS[0]),
            }
        }
    }
}

#[test]
fn the_converter_never_reads_unrendered_storage_after_a_short_loop_block() {
    let (mut control, mut audio, _) = fixture_session(44_100, signal());
    control.send_action(Command::Play { from: 17, end: None, looped: Some(Range::new(17, 20)) }, true).unwrap();
    let mut out = [0.0; 6];
    fill(&mut audio, &mut out);
    // Every render block is the 3-frame loop (frames 17, 18, 19 again and again). Capacity beyond a
    // block's last frame is NOT a valid tap: the taps past it read the next blocks, the loop's start
    // rendered again, so the output is the reader over the looped stream (#525, #601).
    let stream: Vec<Vec<f32>> = (0..2).map(|ch| (0..64).map(|n| ((17 + n % 3) % 31 + 1) as f32 * (ch + 1) as f32 / 128.0).collect()).collect();
    let q = 48_000.0 / 44_100.0;
    for (k, frame) in out.as_chunks::<2>().0.iter().enumerate() {
        for (ch, &got) in frame.iter().enumerate() {
            let mut one = [0.0f32];
            band_limited::frame(band_limited::table(), &Mono(&stream[ch]), band_limited::position(k as i64 * 48_000_000, 44_100_000), q, &mut one);
            // The plan's gain insert at unity, as before #601: within 1e-5 of the reader's value.
            assert!((got - one[0]).abs() < 0.00001, "output frame {k} channel {ch}: {got} against the reader's {}", one[0]);
        }
    }
    assert_eq!(audio.history.misses(), 0);
    control.retire(audio.plans, audio.actions);
}

/// F7 of the #601 review: a new plan with another channel layout, adopted while playing at a
/// converted rate without a restart, never reads the old layout's kept frames into its channels.
/// Here the new plan is a silent 5.1 session: from the output frame that reads its first block on,
/// every output frame is exact silence (without the clear, the old stereo frames still in the taps'
/// reach leaked into the new channels for about 16 output frames).
#[test]
fn a_layout_change_reads_none_of_the_old_layouts_frames() {
    let (mut control, mut audio, _) = fixture_session(44_100, signal());
    control.send_action(Command::Play { from: 0, end: None, looped: None }, true).unwrap();
    let mut out = [0.0f32; 2];
    for _ in 0..600 {
        fill(&mut audio, &mut out);
    }
    assert!(out.iter().any(|x| *x != 0.0), "the old plan plays");
    let old = audio.plans.engine().active().generation;
    control.service(&mut make);
    let mut surround = Session::default();
    surround.outputs[0].format = transit_model::ChannelFormat::Surround51;
    control.request(Arc::new(surround), None).unwrap();
    control.service(&mut make);
    let mut after = Vec::new();
    for _ in 0..3_000 {
        fill(&mut audio, &mut out);
        if audio.plans.engine().active().generation > old {
            after.extend_from_slice(&out);
            if after.len() >= 2 * 64 {
                break;
            }
        }
    }
    assert!(after.len() >= 2 * 64, "the new plan was adopted");
    assert_eq!(audio.plans.output().len(), 6);
    let first = after.iter().position(|x| *x != 0.0);
    assert_eq!(first, None, "an old-layout frame was read after the layout change");
    control.service(&mut make);
    control.retire(audio.plans, audio.actions);
}
