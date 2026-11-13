//! #525: the device-rate converter's output is a function of the rendered stream alone. It used
//! to hold the last frame of every rendered block instead of interpolating into the next block,
//! one wrong sample per block whenever the converter was active.
//!
//! Three layers: the converter's primitives (`Phase`, `Hold`) driven over a synthetic stream cut
//! into arbitrary render blocks, compared bit for bit with a closed-form reference; the live
//! player through its fake device (no real device), with a ramp whose linear interpolation is
//! exact; and the converter's measured quality (passband and alias figures, deterministic tones).
use super::super::phase::{Hold, Phase};
use super::*;
use transit_model::{ChannelFormat, Clip, SourceAudio, SourceId, TrackKind};

const RATES: [u32; 5] = [44_100, 48_000, 88_200, 96_000, 192_000];
/// Ratios no common rate pair has (session, device).
const ODD: [(u32, u32); 2] = [(44_100, 47_999), (47_999, 44_100)];
const PARTITIONS: [&[usize]; 6] = [&[1], &[7], &[64], &[480], &[4096], &[1, 7, 4096, 3, 64, 480, 2, 511, 13]];

fn pairs() -> Vec<(u32, u32)> {
    RATES.iter().flat_map(|&session| RATES.iter().map(move |&device| (session, device))).chain(ODD).collect()
}

/// Output frame `k` read straight from the whole input: frame `floor(k * step / d)` and the next,
/// at the fraction the converter computes (`d` = device rate x 1000, `step` = session rate x speed
/// in thousandths). A whole-frame position is that frame exactly.
fn reference(input: &[f32], session: u32, device: u32, speed: u32, frames: usize) -> Vec<f32> {
    let step = u128::from(session) * u128::from(speed);
    let d = u128::from(device) * 1000;
    (0..frames as u128)
        .map(|k| {
            let units = k * step;
            let index = (units / d) as usize;
            let fraction = ((units % d) as f64 / d as f64) as f32;
            let a = input[index];
            if fraction == 0.0 { a } else { a + (input[index + 1] - a) * fraction }
        })
        .collect()
}

/// The converter's own primitives, in the order `fill_inner` uses them, over `input` rendered in
/// blocks of the sizes in `partition` (repeated).
fn convert(input: &[f32], session: u32, device: u32, speed: u32, partition: &[usize], frames: usize) -> Vec<f32> {
    let step = u64::from(session) * u64::from(speed);
    let mut phase = Phase::new(device).unwrap();
    let mut hold = Hold::new();
    let mut block = vec![vec![0.0f32; 4096]];
    let (mut pending, mut cursor, mut sizes) = (0, 0, partition.iter().cycle());
    let mut out = Vec::with_capacity(frames);
    for _ in 0..frames {
        while phase.needs(pending) {
            hold.keep(&block, pending);
            phase.remove(pending).unwrap();
            pending = *sizes.next().unwrap();
            for (slot, value) in block[0].iter_mut().zip(input.iter().skip(cursor).chain(std::iter::repeat(&0.0))).take(pending) {
                *slot = *value;
            }
            cursor += pending;
        }
        let (index, fraction) = phase.index();
        out.push(hold.sample(block.first(), 0, index, fraction, pending));
        phase.advance(step).unwrap();
    }
    out
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

fn input_frames(session: u32, device: u32, speed: u32, frames: usize) -> usize {
    (frames as u128 * u128::from(session) * u128::from(speed) / (u128::from(device) * 1000)) as usize + 2
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

/// One tone through the converter: (gain at the tone in dB, tone over everything else in dB).
/// `frames` output frames hold a whole number of the tone's cycles and of the converter's phase
/// pattern, so every component sits on a DFT bin (no window, no leakage).
fn tone(session: u32, device: u32, hz: u32, out_hz: u32, frames: usize) -> (f64, f64) {
    use transit_math::f64 as m;
    const AMPLITUDE: f64 = 0.5;
    let tau = 2.0 * core::f64::consts::PI;
    let input: Vec<f32> = (0..input_frames(session, device, 1000, frames))
        .map(|n| (AMPLITUDE * m::sin(tau * f64::from(hz) * n as f64 / f64::from(session))) as f32)
        .collect();
    let out = convert(&input, session, device, 1000, &[1, 7, 4096, 3, 64, 480], frames);
    let (mut re, mut im, mut total) = (0.0, 0.0, 0.0);
    for (n, &y) in out.iter().enumerate() {
        let y = f64::from(y);
        // The bin's angle reduced exactly (integer arithmetic) before the sine.
        let turn = (u64::from(out_hz) * n as u64 % u64::from(device)) as f64 / f64::from(device);
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

/// The linear converter's measured quality, pinned with margin. Linear interpolation is a
/// triangle kernel: its passband droops as the tone nears the session's Nyquist, its images fall
/// off only slowly, and with no anti-alias filter a session rate above the device's folds content
/// above the device's Nyquist back almost unattenuated (an exact 2:1 decimation picks every other
/// frame, so the fold is at full level). These are the figures a user gets until the converter is
/// replaced; the fix record states them.
#[test]
fn the_linear_converter_quality_is_measured_and_pinned() {
    // (session, device, tone, where it is heard on the device, output frames, level there in dB
    // from..to, the tone over everything else at least dB). Where the tone is above the device's
    // Nyquist, "level" is the alias's: its rejection is minus that figure.
    let cases: [(u32, u32, u32, u32, usize, f64, f64, f64); 12] = [
        (44_100, 48_000, 1_000, 1_000, 4800, -0.03, 0.0, 58.0),
        (44_100, 48_000, 10_000, 10_000, 4800, -1.6, -1.4, 19.0),
        (44_100, 48_000, 20_000, 20_000, 4800, -6.5, -6.2, 2.5),
        (48_000, 44_100, 1_000, 1_000, 4410, -0.03, 0.0, 60.0),
        (48_000, 44_100, 10_000, 10_000, 4410, -1.4, -1.1, 20.0),
        (48_000, 44_100, 20_000, 20_000, 4410, -5.5, -5.0, 5.0),
        (48_000, 44_100, 23_000, 21_100, 4410, -7.4, -6.9, 1.0),
        (48_000, 96_000, 1_000, 1_000, 9600, -0.03, 0.0, 57.0),
        (48_000, 96_000, 20_000, 20_000, 9600, -4.2, -3.8, 4.0),
        (96_000, 48_000, 1_000, 1_000, 4800, -0.001, 0.001, 100.0),
        (96_000, 48_000, 20_000, 20_000, 4800, -0.001, 0.001, 100.0),
        (96_000, 48_000, 30_000, 18_000, 4800, -0.001, 0.001, 100.0),
    ];
    let mut failures = Vec::new();
    for (session, device, hz, out_hz, frames, low, high, rest_at_least) in cases {
        let (level, rest) = tone(session, device, hz, out_hz, frames);
        eprintln!("converter quality: {session} -> {device}, {hz} Hz heard at {out_hz} Hz: {level:.3} dB, tone over the rest {rest:.1} dB");
        if !(low..=high).contains(&level) || rest < rest_at_least {
            failures
                .push(format!("{session} -> {device} {hz} Hz: {level:.3} dB (pinned {low}..{high}), rest {rest:.1} dB (pinned >= {rest_at_least})"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// The live player, through its fake device.

/// The ramp's step: a power of two, so `n * STEP` is exact and linear interpolation of the ramp
/// is exact up to one rounding.
const STEP: f32 = 1.0 / 131_072.0;

fn ramp_session(rate: u32, frames: usize) -> (Arc<Session>, [Vec<f32>; 2]) {
    let mut s = Session { sample_rate: transit_time::SampleRate::new(rate).unwrap(), ..Session::default() };
    s.edit.click = false;
    let track = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    let ramp: Vec<f32> = (0..frames).map(|n| n as f32 * STEP).collect();
    // The right channel is a sawtooth pattern with a jump every 31 frames (IEEE-exact values).
    let pattern: Vec<f32> = (0..frames).map(|n| (n % 31) as f32 / 64.0 - 0.25).collect();
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
    control.retire(audio.plans, audio.actions);
    (left, right, input)
}

/// The issue's case: every output frame of a ramp equals the closed form `k * session * speed /
/// device * STEP` within one rounding, and equals the reference interpolation bit for bit.
#[test]
fn a_ramp_through_the_player_is_exact_at_every_output_frame() {
    const FRAMES: usize = 51_200;
    for (session, device, speed) in [(44_100, 48_000, 1000), (48_000, 44_100, 1000), (48_000, 96_000, 1000), (48_000, 48_000, 500)] {
        let (left, right, input) = play(session, device, speed, &[512], FRAMES);
        let mut wrong = 0;
        for (k, &got) in left.iter().enumerate() {
            let position = k as f64 * f64::from(session) * f64::from(speed) / (f64::from(device) * 1000.0);
            let expected = position * f64::from(STEP);
            // One rounding of the sum plus the fraction's own rounding (2^-24 of one step).
            if (f64::from(got) - expected).abs() > expected * f64::from(f32::EPSILON) + f64::from(STEP) * 1e-6 {
                wrong += 1;
            }
        }
        assert_eq!(wrong, 0, "{session} -> {device} at speed {speed}: output frames off the ramp");
        assert_eq!(bits(&left), bits(&reference(&input[0], session, device, speed, FRAMES)), "{session} -> {device} left");
        assert_eq!(bits(&right), bits(&reference(&input[1], session, device, speed, FRAMES)), "{session} -> {device} right");
    }
}

#[test]
fn equal_rates_through_the_player_are_bit_exact_pass_through() {
    for rate in [44_100, 48_000, 96_000] {
        let (left, right, input) = play(rate, rate, 1000, &[512, 7, 1], 9000);
        assert_eq!(bits(&left), bits(&input[0][..9000]), "{rate} Hz left");
        assert_eq!(bits(&right), bits(&input[1][..9000]), "{rate} Hz right");
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
fn the_linear_converter_never_interpolates_from_unrendered_storage_after_a_short_loop_block() {
    let (mut control, mut audio, _) = fixture_session(44_100, signal());
    control.send_action(Command::Play { from: 17, end: None, looped: Some(Range::new(17, 20)) }, true).unwrap();
    let mut out = [0.0; 6];
    fill(&mut audio, &mut out);
    // The third output sample is fractional past the last frame of this 3-frame render (frame
    // 19). Capacity beyond that frame is NOT a valid neighbor: its neighbor is the next block's
    // first frame, the loop's start (frame 17), rendered for it (#525; it used to hold frame 19).
    let fraction = ((2 * 48_000_000 - 2 * 44_100_000) as f64 / 44_100_000.0) as f32;
    let expected = |a: f32, b: f32| a + (b - a) * fraction;
    assert!((out[4] - expected(20.0 / 128.0, 18.0 / 128.0)).abs() < 0.00001, "{}", out[4]);
    assert!((out[5] - expected(40.0 / 128.0, 36.0 / 128.0)).abs() < 0.00001, "{}", out[5]);
    control.retire(audio.plans, audio.actions);
}
