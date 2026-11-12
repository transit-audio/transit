//! Helpers for the tests that drive the processor.

/// A prepared processor.
pub(crate) fn eq(fs: f64, channels: usize) -> crate::ParametricEq {
    let mut e = crate::ParametricEq::new();
    e.prepare(fs, channels);
    e
}

/// Processes `io` in blocks of `block` frames.
pub(crate) fn run(e: &mut crate::ParametricEq, io: &mut [Vec<f32>], block: usize) {
    let frames = io[0].len();
    let mut at = 0;
    while at < frames {
        let n = block.min(frames - at);
        let mut views: Vec<&mut [f32]> = io.iter_mut().map(|c| &mut c[at..at + n]).collect();
        e.process(&mut views, n);
        at += n;
    }
}

/// The processor's fingerprint as one number.
pub(crate) fn fingerprint(e: &crate::ParametricEq) -> u64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    e.fingerprint(&mut h);
    h.finish()
}
