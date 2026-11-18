//! transit's lossless state record (ADR 0029, amendment for #742, section 12): one encoding for
//! the state a testing seam writes, so the transplant gate can compare records exactly and the
//! scheduling gate can hash the same word stream.
//!
//! A record is a sequence of 64-bit words made of items. Each item is a header word (the tag in
//! the top 8 bits, the *logical element count* in the low 56, 1 for a scalar) and its payload:
//!
//! | Tag | Item | Payload words |
//! |---|---|---|
//! | 1 | unsigned integer, up to 64 bits | 1, zero-extended |
//! | 2 | signed integer, up to 64 bits | 1, sign-extended two's complement |
//! | 3 | bool | 1, value 0 or 1 |
//! | 4 | f32 | 1, its bits zero-extended |
//! | 5 | f64 | 1 |
//! | 6 | run of `count` f32 | ceil(count / 2), two to a word, the first in the low half; an odd count's unused final high half is zero |
//! | 7 | run of `count` bytes | ceil(count / 8), little-endian, the last word zero-padded |
//! | 8 | run of `count` unsigned integers | `count` |
//! | 9 | child record | `count` words, the child's whole encoding |
//!
//! An enum is its discriminant (tag 1) followed by that variant's fields; a value with a logical
//! sentinel is a presence bool followed by the value only when present ([`Writer::present`]). A
//! complete plug-in record is two children, the [`Config`] child and the state child
//! ([`Writer::record`]).
//!
//! The [`Writer`] wraps any [`core::hash::Hasher`] and emits every word through
//! [`Hasher::write_u64`] only: the transplant gate's sink keeps every word. The scheduling gate
//! hashes the stream without storing it, through [`Writer::digest`]: the writer feeds its
//! [`Digest`] directly, one inlined mixing step per word with no dynamic call (a dynamic call per
//! word cost the scheduling gate about 13 percent, #742), and the digest is the same value the
//! [`Hasher`] path gives for the same words. A child's header carries its word count, so the
//! writer measures a child first with a counting pass (O(items), not O(state): a run is counted
//! from its length) and then writes it. Nothing allocates.
//!
//! The [`Reader`] restores from a stored record. It fails (`None`) on a tag or count other than
//! the one expected, a short stream, nonzero padding, a bool out of range, a narrowed integer that
//! does not fit, a non-finite float under tags 4 to 6, a child that is not consumed exactly
//! (child exhaustion), or a configuration child that differs from the receiving instance's;
//! [`Reader::finish`] checks top-level exhaustion.
//!
//! The wire format cannot tell a float's `to_bits` written under an integer tag from an integer:
//! the reviewed inventory of each record polices that, and floats go through the typed helpers.

#![no_std]
#![forbid(unsafe_code)]

use core::hash::Hasher;

/// The item tags (see the crate docs).
pub mod tag {
    pub const UINT: u8 = 1;
    pub const INT: u8 = 2;
    pub const BOOL: u8 = 3;
    pub const F32: u8 = 4;
    pub const F64: u8 = 5;
    pub const F32S: u8 = 6;
    pub const BYTES: u8 = 7;
    pub const UINTS: u8 = 8;
    pub const CHILD: u8 = 9;
}

/// The largest logical element count a header holds (56 bits).
pub const MAX_COUNT: u64 = (1 << 56) - 1;

/// The header word of an item.
#[must_use]
pub const fn header(tag: u8, count: u64) -> u64 {
    ((tag as u64) << 56) | (count & MAX_COUNT)
}

/// Two f32 values in one word, the first in the low half.
#[must_use]
pub fn f32_pair(lo: f32, hi: f32) -> u64 {
    u64::from(lo.to_bits()) | (u64::from(hi.to_bits()) << 32)
}

/// The configuration of a complete record (section 12): the registry entry, the sample rate, the
/// maximum block, the channel count and an identifier digest of any loaded content (0 for none).
/// The digest identifies content; it is not a proof of identical configuration (the transplant
/// harness compares the configuration values directly).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config<'a> {
    pub entry: &'a str,
    pub sample_rate: f64,
    pub max_block: u64,
    pub channels: u64,
    pub content: u64,
}

impl Config<'_> {
    /// The configuration child's items.
    pub fn write(&self, w: &mut Writer<'_>) {
        w.bytes(self.entry.as_bytes());
        w.f64(self.sample_rate);
        w.u64(self.max_block);
        w.u64(self.channels);
        w.u64(self.content);
    }

    /// Reads a configuration child's items and checks them against `self`, field by field.
    pub fn matches(&self, r: &mut Reader<'_>) -> Option<()> {
        let n = r.bytes_len()?;
        if n != self.entry.len() || n > 256 {
            return None;
        }
        let mut entry = [0u8; 256];
        let entry = entry.get_mut(..n)?;
        r.bytes(entry)?;
        let same = entry == self.entry.as_bytes()
            && r.f64()?.to_bits() == self.sample_rate.to_bits()
            && r.u64()? == self.max_block
            && r.u64()? == self.channels
            && r.u64()? == self.content;
        same.then_some(())
    }
}

/// The scheduling gate's digest of a word stream, 64 bits, one step per word: `state =
/// mix(state ^ word)` from the FNV offset basis, where `mix` is the SplitMix64 finalizer
/// (xor-shift, multiply, xor-shift, multiply, xor-shift; both multipliers odd). Every operation
/// of `mix` is invertible mod 2^64, so for a fixed state a step is injective in the word and for
/// a fixed word a bijection of the state: two streams that differ in exactly one word always end
/// in different digests. Streams that differ in several words collide with odds near 2^-64
/// under the assumption that digests of different streams behave as independent uniform values
/// (an estimate, not a bound). Deterministic across runs, platforms and Rust versions; compared
/// only within one process run, no value is pinned. (The mix crate's strip print hasher, moved
/// here with #742 so the writer can feed it without a dynamic call.)
#[derive(Clone, Copy, Debug)]
pub struct Digest(u64);

impl Default for Digest {
    fn default() -> Self {
        Digest(0xcbf2_9ce4_8422_2325)
    }
}

impl Digest {
    /// One step over a word (see the type docs).
    #[inline]
    pub fn word(&mut self, w: u64) {
        self.0 = mix(self.0 ^ w);
    }
}

/// The SplitMix64 finalizer: a bijection of `u64` with full avalanche (see [`Digest`]).
#[inline]
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

impl Hasher for Digest {
    /// Eight bytes to a word, little-endian; a short tail is zero-padded, then the length is
    /// hashed, so a tail of zero bytes still counts. (The record writer never calls it.)
    fn write(&mut self, bytes: &[u8]) {
        let (words, tail) = bytes.as_chunks::<8>();
        for &w in words {
            self.word(u64::from_le_bytes(w));
        }
        if !tail.is_empty() {
            let mut last = [0u8; 8];
            if let Some(d) = last.get_mut(..tail.len()) {
                d.copy_from_slice(tail);
            }
            self.word(u64::from_le_bytes(last));
        }
        self.word(bytes.len() as u64);
    }

    fn write_u64(&mut self, i: u64) {
        self.word(i);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// Where a writer's words go.
enum Sink<'a> {
    /// A counting pass: words are counted, not emitted.
    Count,
    /// The scheduling digest, fed directly (no dynamic call per word).
    Digest(&'a mut Digest),
    /// Any hasher (the transplant gate's lossless sink keeps every word).
    Hasher(&'a mut dyn Hasher),
}

/// Writes a record's items into a hasher, word by word (see the crate docs).
pub struct Writer<'a> {
    sink: Sink<'a>,
    words: u64,
}

impl<'a> Writer<'a> {
    /// A writer emitting into `h`.
    pub fn new(h: &'a mut dyn Hasher) -> Self {
        Writer { sink: Sink::Hasher(h), words: 0 }
    }

    /// A writer feeding the scheduling digest `d` directly: the same digest as
    /// `Writer::new(d)` gives, without a dynamic call per word.
    pub fn digest(d: &'a mut Digest) -> Self {
        Writer { sink: Sink::Digest(d), words: 0 }
    }

    /// A writer that only counts the words it would emit.
    #[must_use]
    pub fn counter() -> Writer<'static> {
        Writer { sink: Sink::Count, words: 0 }
    }

    /// Words emitted (or counted) so far.
    #[must_use]
    pub fn words(&self) -> u64 {
        self.words
    }

    /// True on a counting pass (words are counted, not emitted), so a caller may skip computing
    /// a payload whose word count it knows (byte and float runs are skipped this way already).
    /// The items it writes must stay the same: `child` checks the writing pass against the count.
    #[must_use]
    pub fn counting(&self) -> bool {
        matches!(self.sink, Sink::Count)
    }

    #[inline]
    fn word(&mut self, w: u64) {
        self.words += 1;
        match &mut self.sink {
            Sink::Count => {}
            Sink::Digest(d) => d.word(w),
            Sink::Hasher(h) => h.write_u64(w),
        }
    }

    /// Payload words a counting pass skips without visiting them.
    fn skip(&mut self, n: u64) -> bool {
        if matches!(self.sink, Sink::Count) {
            self.words += n;
            return true;
        }
        false
    }

    fn head(&mut self, tag: u8, count: u64) {
        assert!(count <= MAX_COUNT, "a record item of {count} elements");
        self.word(header(tag, count));
    }

    /// Tag 1.
    pub fn u64(&mut self, v: u64) {
        self.head(tag::UINT, 1);
        self.word(v);
    }

    /// Tag 1, widened.
    pub fn usize(&mut self, v: usize) {
        self.u64(v as u64);
    }

    /// Tag 1, widened.
    pub fn u32(&mut self, v: u32) {
        self.u64(u64::from(v));
    }

    /// Tag 1, widened.
    pub fn u16(&mut self, v: u16) {
        self.u64(u64::from(v));
    }

    /// Tag 1, widened.
    pub fn u8(&mut self, v: u8) {
        self.u64(u64::from(v));
    }

    /// Tag 2, two's complement.
    pub fn i64(&mut self, v: i64) {
        self.head(tag::INT, 1);
        self.word(v as u64);
    }

    /// Tag 2, sign-extended.
    pub fn i32(&mut self, v: i32) {
        self.i64(i64::from(v));
    }

    /// Tag 2, sign-extended.
    pub fn i16(&mut self, v: i16) {
        self.i64(i64::from(v));
    }

    /// Tag 2, sign-extended.
    pub fn i8(&mut self, v: i8) {
        self.i64(i64::from(v));
    }

    /// Tag 3.
    pub fn bool(&mut self, v: bool) {
        self.head(tag::BOOL, 1);
        self.word(u64::from(v));
    }

    /// Tag 4.
    pub fn f32(&mut self, v: f32) {
        self.head(tag::F32, 1);
        self.word(u64::from(v.to_bits()));
    }

    /// Tag 5.
    pub fn f64(&mut self, v: f64) {
        self.head(tag::F64, 1);
        self.word(v.to_bits());
    }

    /// Tag 6: the run's values, two to a word.
    pub fn f32s(&mut self, v: &[f32]) {
        self.head(tag::F32S, v.len() as u64);
        if self.skip(v.len().div_ceil(2) as u64) {
            return;
        }
        let (pairs, last) = v.as_chunks::<2>();
        for &[a, b] in pairs {
            self.word(f32_pair(a, b));
        }
        if let [last] = last {
            self.word(u64::from(last.to_bits()));
        }
    }

    /// Tag 7: the bytes, eight to a word, little-endian, the last word zero-padded.
    pub fn bytes(&mut self, v: &[u8]) {
        self.head(tag::BYTES, v.len() as u64);
        if self.skip(v.len().div_ceil(8) as u64) {
            return;
        }
        let (words, tail) = v.as_chunks::<8>();
        for &w in words {
            self.word(u64::from_le_bytes(w));
        }
        if !tail.is_empty() {
            let mut last = [0u8; 8];
            if let Some(d) = last.get_mut(..tail.len()) {
                d.copy_from_slice(tail);
            }
            self.word(u64::from_le_bytes(last));
        }
    }

    /// Tag 8.
    pub fn u64s(&mut self, v: &[u64]) {
        self.uints(v.len(), v.iter().copied());
    }

    /// Tag 8 from `n` values of an iterator (it must yield exactly `n`).
    pub fn uints(&mut self, n: usize, values: impl Iterator<Item = u64>) {
        self.head(tag::UINTS, n as u64);
        if self.skip(n as u64) {
            return;
        }
        let mut seen = 0;
        for v in values {
            seen += 1;
            self.word(v);
        }
        assert_eq!(seen, n, "a run of unsigned integers yielded another count than its header");
    }

    /// Tag 9: the child `f` writes. `f` runs twice (a counting pass, then the writing pass) and
    /// must write the same items both times.
    pub fn child(&mut self, mut f: impl FnMut(&mut Writer<'_>)) {
        let mut count = Writer::counter();
        f(&mut count);
        let n = count.words;
        self.head(tag::CHILD, n);
        if self.skip(n) {
            return;
        }
        let before = self.words;
        f(self);
        assert_eq!(self.words - before, n, "a child record wrote other words than it counted");
    }

    /// A presence bool, then `v`'s items through `f` only when it is present.
    pub fn present<T>(&mut self, v: Option<T>, f: impl FnOnce(&mut Self, T)) {
        self.bool(v.is_some());
        if let Some(v) = v {
            f(self, v);
        }
    }

    /// A complete record: the configuration child, then the state child.
    pub fn record(&mut self, config: &Config<'_>, state: impl FnMut(&mut Writer<'_>)) {
        self.child(|w| config.write(w));
        self.child(state);
    }
}

/// Reads a stored record (see the crate docs). Every read returns `None` on a mismatch; a failed
/// read leaves the reader in an unspecified position, so a restore stops at the first `None`.
#[derive(Clone, Copy, Debug)]
pub struct Reader<'a> {
    words: &'a [u64],
}

impl<'a> Reader<'a> {
    #[must_use]
    pub fn new(words: &'a [u64]) -> Self {
        Reader { words }
    }

    /// Words not consumed yet.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.words.len()
    }

    /// True when every word was consumed (top-level exhaustion).
    #[must_use]
    pub fn finish(self) -> bool {
        self.words.is_empty()
    }

    fn take(&mut self) -> Option<u64> {
        let (&w, rest) = self.words.split_first()?;
        self.words = rest;
        Some(w)
    }

    fn take_n(&mut self, n: usize) -> Option<&'a [u64]> {
        if n > self.words.len() {
            return None;
        }
        let (head, rest) = self.words.split_at(n);
        self.words = rest;
        Some(head)
    }

    /// The next header's count when its tag is `tag`, without consuming it.
    fn peek(&self, tag: u8) -> Option<u64> {
        let &h = self.words.first()?;
        ((h >> 56) as u8 == tag).then_some(h & MAX_COUNT)
    }

    /// Consumes a header of `tag` and `count`.
    fn expect(&mut self, tag: u8, count: u64) -> Option<()> {
        (self.take()? == header(tag, count)).then_some(())
    }

    fn scalar(&mut self, tag: u8) -> Option<u64> {
        self.expect(tag, 1)?;
        self.take()
    }

    /// Tag 1.
    pub fn u64(&mut self) -> Option<u64> {
        self.scalar(tag::UINT)
    }

    /// Tag 1, narrowed (fails when the value does not fit).
    pub fn usize(&mut self) -> Option<usize> {
        usize::try_from(self.u64()?).ok()
    }

    /// Tag 1, narrowed.
    pub fn u32(&mut self) -> Option<u32> {
        u32::try_from(self.u64()?).ok()
    }

    /// Tag 1, narrowed.
    pub fn u16(&mut self) -> Option<u16> {
        u16::try_from(self.u64()?).ok()
    }

    /// Tag 1, narrowed.
    pub fn u8(&mut self) -> Option<u8> {
        u8::try_from(self.u64()?).ok()
    }

    /// Tag 2.
    pub fn i64(&mut self) -> Option<i64> {
        self.scalar(tag::INT).map(|w| w as i64)
    }

    /// Tag 2, narrowed.
    pub fn i32(&mut self) -> Option<i32> {
        i32::try_from(self.i64()?).ok()
    }

    /// Tag 2, narrowed.
    pub fn i16(&mut self) -> Option<i16> {
        i16::try_from(self.i64()?).ok()
    }

    /// Tag 2, narrowed.
    pub fn i8(&mut self) -> Option<i8> {
        i8::try_from(self.i64()?).ok()
    }

    /// Tag 3 (0 or 1 only).
    pub fn bool(&mut self) -> Option<bool> {
        match self.scalar(tag::BOOL)? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }

    /// Tag 4 (finite only; the high half must be zero).
    pub fn f32(&mut self) -> Option<f32> {
        let w = self.scalar(tag::F32)?;
        let v = f32::from_bits(u32::try_from(w).ok()?);
        v.is_finite().then_some(v)
    }

    /// Tag 5 (finite only).
    pub fn f64(&mut self) -> Option<f64> {
        let v = f64::from_bits(self.scalar(tag::F64)?);
        v.is_finite().then_some(v)
    }

    /// The count of the tag-6 run that comes next, unconsumed.
    #[must_use]
    pub fn f32s_len(&self) -> Option<usize> {
        usize::try_from(self.peek(tag::F32S)?).ok()
    }

    /// Tag 6 into `out`, whose length must be the run's count (every value finite).
    pub fn f32s(&mut self, out: &mut [f32]) -> Option<()> {
        self.expect(tag::F32S, out.len() as u64)?;
        let words = self.take_n(out.len().div_ceil(2))?;
        let (pairs, last) = out.as_chunks_mut::<2>();
        let mut it = words.iter();
        for pair in pairs {
            let w = *it.next()?;
            *pair = [f32::from_bits(w as u32), f32::from_bits((w >> 32) as u32)];
        }
        if let [last] = last {
            let w = *it.next()?;
            if w >> 32 != 0 {
                return None;
            }
            *last = f32::from_bits(w as u32);
        }
        out.iter().all(|v| v.is_finite()).then_some(())
    }

    /// The count of the tag-7 run that comes next, unconsumed.
    #[must_use]
    pub fn bytes_len(&self) -> Option<usize> {
        usize::try_from(self.peek(tag::BYTES)?).ok()
    }

    /// Tag 7 into `out`, whose length must be the run's count (the padding must be zero).
    pub fn bytes(&mut self, out: &mut [u8]) -> Option<()> {
        self.expect(tag::BYTES, out.len() as u64)?;
        let words = self.take_n(out.len().div_ceil(8))?;
        let (chunks, tail) = out.as_chunks_mut::<8>();
        let mut it = words.iter();
        for c in chunks {
            *c = it.next()?.to_le_bytes();
        }
        if !tail.is_empty() {
            let last = it.next()?.to_le_bytes();
            let (used, pad) = last.split_at(tail.len());
            if pad.iter().any(|&b| b != 0) {
                return None;
            }
            tail.copy_from_slice(used);
        }
        Some(())
    }

    /// The count of the tag-8 run that comes next, unconsumed.
    #[must_use]
    pub fn u64s_len(&self) -> Option<usize> {
        usize::try_from(self.peek(tag::UINTS)?).ok()
    }

    /// Tag 8 into `out`, whose length must be the run's count.
    pub fn u64s(&mut self, out: &mut [u64]) -> Option<()> {
        self.expect(tag::UINTS, out.len() as u64)?;
        out.copy_from_slice(self.take_n(out.len())?);
        Some(())
    }

    /// Tag 9: `f` reads the child, which it must consume exactly.
    pub fn child<T>(&mut self, f: impl FnOnce(&mut Reader<'a>) -> Option<T>) -> Option<T> {
        let n = usize::try_from(self.peek(tag::CHILD)?).ok()?;
        self.take()?;
        let mut inner = Reader { words: self.take_n(n)? };
        let v = f(&mut inner)?;
        inner.words.is_empty().then_some(v)
    }

    /// A presence bool, then the value through `f` only when present.
    pub fn present<T>(&mut self, f: impl FnOnce(&mut Self) -> Option<T>) -> Option<Option<T>> {
        if self.bool()? { f(self).map(Some) } else { Some(None) }
    }

    /// A complete record: the configuration child, which must match `config`, then the state
    /// child through `state`.
    pub fn record<T>(&mut self, config: &Config<'_>, state: impl FnOnce(&mut Reader<'a>) -> Option<T>) -> Option<T> {
        self.child(|r| config.matches(r))?;
        self.child(state)
    }
}

#[cfg(test)]
mod tests;
