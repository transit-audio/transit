//! The correlator of the pitch shifter and time stretcher (`clouds/dsp/correlator.{h,cc}`): it
//! finds where a new window lines up best with the one playing by XOR-ing the sign bits of the two
//! stretches of audio, 32 samples per word, and keeps the candidate with the most matching bits.
//!
//! The search runs a few candidates at a time: the processor's `Prepare` evaluates a quarter of
//! them (plus 16) after every block, as the firmware's desktop harness orders it, and the player
//! takes whatever the best match is when its next window is due.

#![forbid(unsafe_code)]

/// `kMaxWSOLASize / 32 + 2`: the words of one correlator block. The source takes one block; the
/// destination (twice as long a stretch) the next two.
pub(crate) const BLOCK_WORDS: usize = 4096 / 32 + 2;

/// `Correlator`, with its two blocks of words (the firmware's are in the FX workspace).
///
/// The words are not cleared by [`Correlator::init`], as in the C++: a search reads one word past
/// the destination it was loaded with (the last word's shifted-in bits), which then holds what an
/// earlier, longer search left there. The processor clears them where the firmware's memory is
/// cleared: when the workspace is set up (the pitch shifter's memory, initialized over the
/// correlator's) and on a change between two benign playback modes (that memory's `Clear`).
#[derive(Clone, Debug)]
pub(crate) struct Correlator {
    source: [u32; BLOCK_WORDS],
    destination: [u32; 2 * BLOCK_WORDS],
    offset: i32,
    increment: i32,
    size: i32,
    candidate: i32,
    best_score: u32,
    best_match: i32,
    done: bool,
}

impl Default for Correlator {
    fn default() -> Self {
        Self {
            source: [0; BLOCK_WORDS],
            destination: [0; 2 * BLOCK_WORDS],
            offset: 0,
            increment: 0,
            size: 0,
            candidate: 0,
            best_score: 0,
            best_match: 0,
            done: false,
        }
    }
}

impl Correlator {
    /// `Init(source, destination)`: no search, no match. The other fields keep their values, as in
    /// the C++ (none is read before the next [`Correlator::start_search`]).
    pub(crate) fn init(&mut self) {
        self.offset = 0;
        self.best_match = 0;
        self.done = true;
    }

    /// The firmware's memory under the correlator cleared (the pitch shifter's `Init` or `Clear`
    /// over the same words).
    pub(crate) fn clear_memory(&mut self) {
        self.source.fill(0);
        self.destination.fill(0);
    }

    /// `source()`: the words the source stretch's sign bits are loaded into.
    pub(crate) fn source_mut(&mut self) -> &mut [u32] {
        &mut self.source
    }

    /// `destination()`: the words the destination stretch's sign bits are loaded into.
    pub(crate) fn destination_mut(&mut self) -> &mut [u32] {
        &mut self.destination
    }

    /// `StartSearch(size, offset, increment)`.
    pub(crate) fn start_search(&mut self, size: i32, offset: i32, increment: i32) {
        self.offset = offset;
        self.increment = increment;
        self.best_score = 0;
        self.best_match = 0;
        self.candidate = 0;
        self.size = size;
        self.done = false;
    }

    /// `best_match()`: the best candidate so far, as a position in the recording.
    pub(crate) fn best_match(&self) -> i32 {
        self.offset.wrapping_add(self.best_match.wrapping_mul(self.increment >> 4) >> 12)
    }

    /// `EvaluateSomeCandidates()`: a quarter of the candidates plus 16.
    pub(crate) fn evaluate_some_candidates(&mut self) {
        let num_candidates = (self.size >> 2) + 16;
        for _ in 0..num_candidates {
            self.evaluate_next_candidate();
        }
    }

    /// The word at `index`, or 0 past the block (never reached: the loads fit their blocks).
    fn word(words: &[u32], index: u32) -> u32 {
        usize::try_from(index).ok().and_then(|i| words.get(i)).copied().unwrap_or(0)
    }

    /// `EvaluateNextCandidate()`. The C++ shifts a 32-bit word right by 32 when the candidate is a
    /// multiple of 32 (undefined behavior); the oracle pins that to 0, as the module's Cortex-M4
    /// computes it (`patches/all/0002` in the oracle crate), and so does the port.
    fn evaluate_next_candidate(&mut self) {
        if self.done {
            return;
        }
        #[allow(clippy::cast_sign_loss)]
        let (num_words, offset_words, offset_bits) = ((self.size >> 5) as u32, (self.candidate >> 5) as u32, (self.candidate & 0x1f) as u32);
        let mut xcorr = 0_u32;
        for i in 0..num_words {
            let source_bits = Self::word(&self.source, i);
            let mut destination_bits = Self::word(&self.destination, offset_words + i) << offset_bits;
            if offset_bits != 0 {
                destination_bits |= Self::word(&self.destination, offset_words + i + 1) >> (32 - offset_bits);
            }
            // The C++'s bit-twiddling population count of the matching bits.
            xcorr = xcorr.wrapping_add((!(source_bits ^ destination_bits)).count_ones());
        }
        if xcorr > self.best_score {
            self.best_match = self.candidate;
            self.best_score = xcorr;
        }
        self.candidate += 1;
        self.done = self.candidate >= self.size;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_search_finds_a_shifted_copy() {
        let mut c = Correlator::default();
        c.init();
        // A pseudo-random bit pattern, and the same pattern 37 bits later in the destination.
        let mut state = 0x1234_5678_u32;
        let bits: Vec<bool> = (0..64 * 32)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                state >> 31 != 0
            })
            .collect();
        let pack = |bits: &[bool], words: &mut [u32]| {
            for (w, chunk) in words.iter_mut().zip(bits.chunks(32)) {
                *w = chunk.iter().fold(0, |acc, b| (acc << 1) | u32::from(*b));
            }
        };
        pack(&bits[37..37 + 32 * 16], c.source_mut());
        pack(&bits[..32 * 34], c.destination_mut());
        c.start_search(32 * 16, 1000, 1 << 16);
        while !c.done {
            c.evaluate_some_candidates();
        }
        assert_eq!(c.best_match, 37);
        assert_eq!(c.best_match(), 1000 + 37);
    }
}
