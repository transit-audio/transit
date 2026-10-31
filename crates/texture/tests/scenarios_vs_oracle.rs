//! Scenarios against the oracle (#241), bit for bit: quality and mono changes mid-stream, every
//! block size, the dry path, trims and the gate output, the shortest and the longest buffers, and
//! the inputs where the firmware is undefined (defined by `patches/all/0005`): a clock stretched
//! past the buffer and a SEED clock at audio rate in the frozen delay mode. Also the evidence that
//! the float output tells the flush to zero apart, so the port's flush is checked bit for bit.

#![forbid(unsafe_code)]

mod common;

use transit_macro_osc_oracle::texture::{Controls, Media, SeedButton, TextureOracle};
use transit_macro_osc_oracle::texture_sweep::{center, input_signal};
use transit_macro_osc_oracle::{Error, Session};
use transit_texture::stmlib::extract_gate_flags;
use transit_texture::{BufferSize, Texture};

/// One block's settings.
struct Step {
    controls: Controls,
    freeze: bool,
    /// The SEED gate per frame of the block, or `None` when nothing is patched.
    seed: Option<Vec<bool>>,
    /// Before the block: `configure(media, mono)`.
    configure: Option<(Media, bool)>,
    /// Before the block: `trim()`.
    trim: bool,
}

impl Step {
    fn new(controls: Controls) -> Self {
        Self { controls, freeze: false, seed: None, configure: None, trim: false }
    }
}

/// Runs the same steps through the port and the oracle; asserts the outputs and the random
/// generators' final states are equal; returns the oracle's output.
fn run(name: &str, port: &mut Texture, oracle: &mut TextureOracle<'_>, session: &Session, blocks: &[(usize, Step)]) -> Vec<[f32; 2]> {
    let total: usize = blocks.iter().map(|(n, _)| n).sum();
    let input = input_signal(total);
    let mut expected = vec![[0.0_f32; 2]; total];
    let mut got = vec![[0.0_f32; 2]; total];
    let mut start = 0;
    let mut previous_gate = false;
    for (n, step) in blocks {
        if let Some((media, mono)) = step.configure {
            oracle.configure(media, mono);
            port.configure(common::media(media), mono);
        }
        if step.trim {
            oracle.trim();
            port.trim();
        }
        let freeze = vec![step.freeze; *n];
        let flags: Option<Vec<u8>> = step.seed.as_ref().map(|gates| {
            gates
                .iter()
                .map(|&g| {
                    let f = extract_gate_flags(u8::from(previous_gate), g);
                    previous_gate = g;
                    f
                })
                .collect()
        });
        let range = start..start + n;
        oracle.process(&step.controls, &freeze, flags.as_deref(), &input[range.clone()], &mut expected[range.clone()]).unwrap();
        port.process(&common::controls(&step.controls), &freeze, flags.as_deref(), &input[range.clone()], &mut got[range]);
        start += n;
    }
    let r = common::residual(&expected, &got);
    assert!(r.is_exact(), "{name}: {r}");
    assert_eq!(port.rng_state(), session.rng_state(), "{name}: the random generators end in the same state");
    expected
}

/// Granular controls that sound early on a fresh buffer.
fn near() -> Controls {
    let mut c = center();
    c.time.base = 0.02;
    c.density = 0.9;
    c
}

fn pair(session: &Session, buffer: BufferSize, media: Media, mono: bool, seed: u32) -> (Texture, TextureOracle<'_>) {
    let mut port = Texture::new(buffer, common::media(media), mono);
    port.seed(seed);
    session.seed(seed);
    let oracle = if buffer == BufferSize::FIRMWARE { session.texture(media, mono) } else { session.texture_long(media, mono) };
    (port, oracle.unwrap())
}

#[test]
fn quality_and_mono_changes_mid_stream() {
    let session = Session::lock();
    let (mut port, mut oracle) = pair(&session, BufferSize::FIRMWARE, Media::Digital, false, 11);
    let mut blocks = Vec::new();
    let qualities = [
        (Media::Digital, false),
        (Media::VintageDigital, true),
        (Media::CleanTape, false),
        (Media::DirtyTape, true),
        (Media::Digital, true),
        (Media::DirtyTape, false),
        (Media::CleanTape, true),
        (Media::VintageDigital, false),
    ];
    for (k, &(media, mono)) in qualities.iter().enumerate() {
        let block = media.block_size();
        let count = media.sample_rate() as usize * 3 / 2 / block;
        for b in 0..count {
            let mut c = near();
            // Segments 4 and 5 cross into the delay mode (the crossing survives a configure).
            if k == 4 || k == 5 {
                c.size.base = 1.0;
            }
            let mut step = Step::new(c);
            step.freeze = k == 2 && b > count / 2;
            if b == 0 && k > 0 {
                step.configure = Some((media, mono));
            }
            blocks.push((block, step));
        }
    }
    run("quality and mono changes", &mut port, &mut oracle, &session, &blocks);
}

#[test]
fn every_block_size() {
    let session = Session::lock();
    for media in Media::ALL {
        let (mut port, mut oracle) = pair(&session, BufferSize::FIRMWARE, media, false, 12);
        let sizes: Vec<usize> = (1..=24).filter(|n| media != Media::CleanTape || n % 2 == 0).collect();
        let mut blocks = Vec::new();
        let mut frames = 0;
        let mut k = 0;
        while frames < media.sample_rate() as usize * 3 {
            let n = sizes[k % sizes.len()];
            let mut c = near();
            c.size.base = if frames > media.sample_rate() as usize { 1.0 } else { 0.4 };
            let mut step = Step::new(c);
            step.freeze = (k / 40) % 3 == 2;
            blocks.push((n, step));
            frames += n;
            k += 1;
        }
        run(&format!("every block size, {media:?}"), &mut port, &mut oracle, &session, &blocks);
    }
}

#[test]
fn the_dry_path_trims_and_the_gate_output() {
    let session = Session::lock();
    for media in Media::ALL {
        for mono in [false, true] {
            let (mut port, mut oracle) = pair(&session, BufferSize::FIRMWARE, media, mono, 13);
            let block = media.block_size();
            let count = media.sample_rate() as usize * 4 / block;
            let blocks: Vec<(usize, Step)> = (0..count)
                .map(|b| {
                    let mut c = near();
                    c.dry_wet = (b as f32 / count as f32 * 2.0).min(1.0);
                    c.output_gate = b > count * 3 / 4;
                    let mut step = Step::new(c);
                    step.freeze = (count / 3..count / 2).contains(&b);
                    step.trim = b == count / 4 || b == count * 5 / 12;
                    (block, step)
                })
                .collect();
            run(&format!("dry, trims and gate, {media:?} mono {mono}"), &mut port, &mut oracle, &session, &blocks);
        }
    }
}

/// TIME across the whole buffer (deep positions read the recording after it wraps), then the
/// frozen delay mode, on the module's buffer and on the longest.
#[test]
fn the_shortest_and_the_longest_buffers() {
    let session = Session::lock();
    for buffer in [BufferSize::MIN, BufferSize::MAX] {
        for (media, mono) in [(Media::Digital, false), (Media::CleanTape, true), (Media::DirtyTape, true), (Media::VintageDigital, false)] {
            let (mut port, mut oracle) = pair(&session, buffer, media, mono, 14);
            let block = media.block_size();
            let rate = media.sample_rate() as usize;
            // Long enough for the recording to wrap at TIME 1: the buffer at this quality plus 2 s.
            let seconds = buffer.seconds()
                * if mono { 2.0 } else { 1.0 }
                * match media {
                    Media::Digital => 1.0,
                    Media::VintageDigital => 2.0,
                    Media::CleanTape => 8.0 / 3.0,
                    Media::DirtyTape => 4.0,
                }
                + 2.0;
            let count = (seconds * rate as f32) as usize / block;
            let blocks: Vec<(usize, Step)> = (0..count)
                .map(|b| {
                    let t = (b * block) as f32 / rate as f32;
                    let mut c = near();
                    c.time.base = if t + 4.0 < seconds { (t / 4.0).fract() } else { 0.9 };
                    if t + 3.0 >= seconds {
                        c.size.base = 1.0;
                    }
                    let mut step = Step::new(c);
                    step.freeze = t + 1.0 >= seconds;
                    (block, step)
                })
                .collect();
            run(&format!("{buffer:?} {media:?} mono {mono}"), &mut port, &mut oracle, &session, &blocks);
        }
    }
}

/// The firmware's undefined cases, as `patches/all/0005` defines them: with the SEED button latched
/// and nothing patched, a density just above 0.76 and its CV at -1 stretch the clock interval to
/// about 13 times the buffer (each slice is then the whole buffer); and a SEED clock every other
/// sample gives slices shorter than one sample (zero slices: silent play heads). Both in the frozen
/// delay mode, at the longest buffer too (the stretched slices are where the fixed-point delays
/// come closest to their limit).
#[test]
fn the_undefined_slices_are_defined_as_the_oracle_defines_them() {
    let session = Session::lock();
    for buffer in [BufferSize::MIN, BufferSize::MAX] {
        for (media, mono) in [
            (Media::Digital, false),
            (Media::Digital, true),
            (Media::VintageDigital, false),
            (Media::VintageDigital, true),
            (Media::CleanTape, false),
            (Media::CleanTape, true),
            (Media::DirtyTape, false),
            (Media::DirtyTape, true),
        ] {
            let (mut port, mut oracle) = pair(&session, buffer, media, mono, 15);
            let block = media.block_size();
            let rate = media.sample_rate() as usize;
            let count = rate * 8 / block;
            let blocks: Vec<(usize, Step)> = (0..count)
                .map(|b| {
                    let t = (b * block) as f32 / rate as f32;
                    let mut c = near();
                    c.size.base = 1.0;
                    let mut step = Step::new(c);
                    step.freeze = t >= 3.0;
                    if t < 5.0 {
                        step.controls.density = 0.762;
                        step.controls.density_cv = -1.0;
                    } else {
                        step.controls.seed_button = SeedButton::Off;
                        step.controls.density = 0.1;
                        step.seed = Some((0..block).map(|k| (b * block + k) % 2 == 0).collect());
                    }
                    (block, step)
                })
                .collect();
            run(&format!("undefined slices, {buffer:?} {media:?} mono {mono}"), &mut port, &mut oracle, &session, &blocks);
        }
    }
}

/// Reconfigures while the buffer is frozen: a mono toggle, then a quality change (and back), in the
/// granular mode and in the delay mode (SIZE 1), at both buffer sizes. Each segment lasts 1.5 s at
/// its quality's rate; the freeze starts at 3 s and the first change comes at 4.5 s.
#[test]
fn reconfiguring_while_frozen() {
    let session = Session::lock();
    for buffer in [BufferSize::FIRMWARE, BufferSize::MAX] {
        for delay_mode in [false, true] {
            for (start, mono) in [(Media::Digital, false), (Media::CleanTape, true), (Media::VintageDigital, false)] {
                let (mut port, mut oracle) = pair(&session, buffer, start, mono, 17);
                let other = if start == Media::DirtyTape { Media::Digital } else { Media::DirtyTape };
                // (media, mono) in force from each segment on: filled (2 x), frozen, mono toggled,
                // quality changed, quality restored.
                let segments = [
                    (start, mono, false),
                    (start, mono, false),
                    (start, mono, true),
                    (start, !mono, true),
                    (other, !mono, true),
                    (start, mono, true),
                ];
                let mut blocks = Vec::new();
                for (k, &(media, mono, freeze)) in segments.iter().enumerate() {
                    let block = media.block_size();
                    let count = media.sample_rate() as usize * 3 / 2 / block;
                    for b in 0..count {
                        let mut c = near();
                        if delay_mode {
                            c.size.base = 1.0;
                        }
                        let mut step = Step::new(c);
                        step.freeze = freeze;
                        if b == 0 && k >= 3 {
                            step.configure = Some((media, mono));
                        }
                        blocks.push((block, step));
                    }
                }
                run(
                    &format!("reconfiguring while frozen, {buffer:?} {start:?} mono {mono} delay mode {delay_mode}"),
                    &mut port,
                    &mut oracle,
                    &session,
                    &blocks,
                );
            }
        }
    }
}

/// The output is float, so the flush to zero is visible in its bits: rendered without it, the
/// oracle's tails differ (subnormal values instead of zeros), and the port, which flushes as the
/// oracle does by default, matches the flushed reference exactly. A port that lost its flush would
/// fail this test, and the long runs.
#[test]
fn the_flush_to_zero_is_visible_and_reproduced() {
    let session = Session::lock();
    let render = |flush: bool| -> Result<Vec<[f32; 2]>, Error> {
        session.seed(16);
        let mut oracle = session.texture(Media::Digital, false)?;
        oracle.set_flush_to_zero(flush);
        let rate = 48_000;
        let input: Vec<[f32; 2]> = input_signal(rate * 2).into_iter().chain(std::iter::repeat_n([0.0; 2], rate * 8)).collect();
        let mut out = vec![[0.0_f32; 2]; input.len()];
        let mut c = near();
        c.reverb = 0.7;
        c.feedback = 0.6;
        for (i, o) in input.chunks(12).zip(out.chunks_mut(12)) {
            oracle.process(&c, &[false; 12], None, i, o)?;
        }
        Ok(out)
    };
    let flushed = render(true).unwrap();
    let unflushed = render(false).unwrap();
    let subnormals = unflushed.iter().flatten().filter(|s| s.is_subnormal()).count();
    assert!(subnormals > 0, "the unflushed render reaches the subnormal range");
    assert!(flushed.iter().flatten().all(|s| !s.is_subnormal()), "the flushed render never holds a subnormal");
    let r = common::residual(&flushed, &unflushed);
    assert!(!r.is_exact(), "the flush must change the output's bits");
    println!("flushed against unflushed: {r}, {subnormals} subnormal samples");

    let mut port = Texture::new(BufferSize::FIRMWARE, transit_texture::Media::Digital, false);
    port.seed(16);
    let rate = 48_000;
    let input: Vec<[f32; 2]> = input_signal(rate * 2).into_iter().chain(std::iter::repeat_n([0.0; 2], rate * 8)).collect();
    let mut got = vec![[0.0_f32; 2]; input.len()];
    let mut c = near();
    c.reverb = 0.7;
    c.feedback = 0.6;
    let c = common::controls(&c);
    for (i, o) in input.chunks(12).zip(got.chunks_mut(12)) {
        port.process(&c, &[false; 12], None, i, o);
    }
    let r = common::residual(&flushed, &got);
    assert!(r.is_exact(), "the port against the flushed oracle: {r}");
}
