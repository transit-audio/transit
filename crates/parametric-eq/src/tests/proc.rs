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

/// The processor's state record, every word kept (the comparisons are exact; #742 migrated the
/// sink from a hash of the same fields).
pub(crate) fn fingerprint(e: &crate::ParametricEq) -> Vec<u64> {
    #[derive(Default)]
    struct Words(Vec<u64>);
    impl core::hash::Hasher for Words {
        fn write(&mut self, _: &[u8]) {
            unreachable!("the record writer emits words only")
        }
        fn write_u64(&mut self, w: u64) {
            self.0.push(w);
        }
        fn finish(&self) -> u64 {
            0
        }
    }
    let mut h = Words::default();
    e.fingerprint(&mut transit_state_record::Writer::new(&mut h));
    h.0
}
