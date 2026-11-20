//! Test seam (feature `testing`; ADR 0029, amendment for #742, sections 12 and 13): the
//! processor's complete state record and its restore, over the `transit-state-record` codec. Never
//! on the audio path, and compiled only with the feature (the codec, the crate's only workspace
//! dependency, is optional behind it).
//!
//! The same machinery as the macro oscillator's and the resonator's port records: every type
//! implements [`Record`] in its defining module, so private fields are reachable;
//! [`record_struct!`] writes every function from one exhaustive destructuring of `Self { .. }` with
//! no rest pattern, so a new field does not compile until its treatment is decided;
//! [`record_enum!`] matches every variant. A field is encoded, in declaration order, or named in
//! the macro's `config` list (configuration: what the constructor fixes), its `unobservable` list
//! (an overwrite-before-read argument and a poison witness at the call site), or its `derived`
//! list. (The `reset` lists of the shared macro are unused here.)
//!
//! Runs: an `f32` or [`F`](crate::float::F) run is a count, then tag-6 items of at most [`CHUNK`]
//! values; `u8`, `i8`, `u16` and `i16` runs are packed little-endian into tag-7 items of at most
//! [`CHUNK`] bytes behind a count (integers, never floats, under the byte tag); a `u32` run is one
//! tag-8 item; anything else is a count then each element. Restore checks every length against the
//! receiving instance's (lengths are configuration: the processor allocates nothing after
//! construction) and fails on any mismatch.

pub use transit_state_record::{Reader, Writer};

/// A value of the voice's state: written to a record, and set from one.
pub trait Record {
    /// Writes the value's items.
    fn record(&self, w: &mut Writer<'_>);

    /// Sets the value from the items [`Record::record`] wrote; `None` on any mismatch (the reader
    /// is then in an unspecified position, so the caller stops).
    fn restore(&mut self, r: &mut Reader<'_>) -> Option<()>;

    /// A run of values (an array's or a buffer's elements): a count, then each element; a type
    /// with a packed form overrides it.
    fn record_run(items: &[Self], w: &mut Writer<'_>)
    where
        Self: Sized,
    {
        w.usize(items.len());
        items.iter().for_each(|x| x.record(w));
    }

    /// Restores a run [`Record::record_run`] wrote into `items`, whose length must match.
    fn restore_run(items: &mut [Self], r: &mut Reader<'_>) -> Option<()>
    where
        Self: Sized,
    {
        (r.usize()? == items.len()).then_some(())?;
        items.iter_mut().try_for_each(|x| x.restore(r))
    }

    /// Writes the value as it is recorded while the engine owning it is dormant (see the module
    /// docs): by default, everything.
    fn record_dormant(&self, w: &mut Writer<'_>) {
        self.record(w);
    }

    /// Restores what [`Record::record_dormant`] wrote, leaving what it left out.
    fn restore_dormant(&mut self, r: &mut Reader<'_>) -> Option<()> {
        self.restore(r)
    }

    /// Poison witnesses only: overwrites every encoded field (left-out ones are not touched) with
    /// a finite value that differs from the one it replaces. A type no witness poisons whole keeps
    /// this default, which panics.
    fn poison(&mut self, p: &mut Poison) {
        let _ = p;
        unimplemented!("{} has no poison", core::any::type_name::<Self>());
    }

    /// Poison witnesses only: poisons what [`Record::record_dormant`] leaves out (by default,
    /// nothing).
    fn poison_dormant(&mut self, p: &mut Poison) {
        let _ = p;
    }
}

/// The poison witnesses' value source: every value it gives differs from the value it replaces,
/// and every float is finite and positive, so a witness that poisons a field changes its contents.
#[derive(Debug)]
pub struct Poison {
    state: u32,
    count: u64,
}

impl Poison {
    /// A source starting from `seed`.
    #[must_use]
    pub fn new(seed: u32) -> Self {
        Poison { state: seed.wrapping_mul(0x9e37_79b9) | 1, count: 0 }
    }

    /// Values written so far.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.count
    }

    fn next(&mut self) -> u32 {
        self.count += 1;
        self.state = self.state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.state >> 8
    }

    /// A step from 1 to 8 (an integer's poison adds it, wrapping, so the value always changes).
    #[must_use]
    pub fn step(&mut self) -> u8 {
        1 + (self.next() & 7) as u8
    }

    /// A finite positive float below 1 other than `old`.
    #[must_use]
    pub fn f32(&mut self, old: f32) -> f32 {
        let v = 0.0625 + self.next() as f32 / 33_554_432.0;
        if v.to_bits() == old.to_bits() { v + 0.25 } else { v }
    }
}

/// The largest tag-7 item a packed run writes (a stack buffer: nothing allocates).
pub const CHUNK: usize = 256;

/// Writes `items` packed `N` bytes each by `pack`: a count of bytes, then tag-7 items of whole
/// items, at most [`CHUNK`] bytes each (`N` is at most [`CHUNK`]).
pub fn write_packed<T, const N: usize>(w: &mut Writer<'_>, items: &[T], pack: impl Fn(&T) -> [u8; N]) {
    const { assert!(N > 0 && N <= CHUNK) };
    w.usize(items.len() * N);
    let mut buf = [0u8; CHUNK];
    if w.counting() {
        // The same items, unpacked: a counting pass reads only each chunk's length.
        for chunk in items.chunks(CHUNK / N) {
            w.bytes(buf.get(..chunk.len() * N).unwrap_or_default());
        }
        return;
    }
    for chunk in items.chunks(CHUNK / N) {
        let mut n = 0;
        for (it, dst) in chunk.iter().zip(buf.as_chunks_mut::<N>().0) {
            *dst = pack(it);
            n += N;
        }
        w.bytes(buf.get(..n).unwrap_or_default());
    }
}

/// Reads what [`write_packed`] wrote into `items` (its length must match), each through `unpack`.
pub fn read_packed<T, const N: usize>(r: &mut Reader<'_>, items: &mut [T], unpack: impl Fn(&mut T, [u8; N])) -> Option<()> {
    const { assert!(N > 0 && N <= CHUNK) };
    (r.usize()? == items.len() * N).then_some(())?;
    let mut buf = [0u8; CHUNK];
    for chunk in items.chunks_mut(CHUNK / N) {
        let bytes = buf.get_mut(..chunk.len() * N)?;
        r.bytes(bytes)?;
        for (it, src) in chunk.iter_mut().zip(bytes.as_chunks::<N>().0) {
            unpack(it, *src);
        }
    }
    Some(())
}

impl Record for f32 {
    fn record(&self, w: &mut Writer<'_>) {
        w.f32(*self);
    }
    fn restore(&mut self, r: &mut Reader<'_>) -> Option<()> {
        *self = r.f32()?;
        Some(())
    }
    fn record_run(items: &[Self], w: &mut Writer<'_>) {
        w.f32s(items);
    }
    fn restore_run(items: &mut [Self], r: &mut Reader<'_>) -> Option<()> {
        r.f32s(items)
    }
    fn poison(&mut self, p: &mut Poison) {
        *self = p.f32(*self);
    }
}

/// The port's flush-to-zero float: recorded as its `f32` (tag 4), runs in chunks of tag-6 items.
impl Record for crate::float::F {
    fn record(&self, w: &mut Writer<'_>) {
        w.f32(self.0);
    }
    fn restore(&mut self, r: &mut Reader<'_>) -> Option<()> {
        self.0 = r.f32()?;
        Some(())
    }
    fn record_run(items: &[Self], w: &mut Writer<'_>) {
        w.usize(items.len());
        let mut buf = [0.0f32; CHUNK];
        for chunk in items.chunks(CHUNK) {
            for (d, s) in buf.iter_mut().zip(chunk) {
                *d = s.0;
            }
            w.f32s(buf.get(..chunk.len()).unwrap_or_default());
        }
    }
    fn restore_run(items: &mut [Self], r: &mut Reader<'_>) -> Option<()> {
        (r.usize()? == items.len()).then_some(())?;
        let mut buf = [0.0f32; CHUNK];
        for chunk in items.chunks_mut(CHUNK) {
            let b = buf.get_mut(..chunk.len())?;
            r.f32s(b)?;
            for (d, s) in chunk.iter_mut().zip(b.iter()) {
                d.0 = *s;
            }
        }
        Some(())
    }
    fn poison(&mut self, p: &mut Poison) {
        self.0 = p.f32(self.0);
    }
}

impl Record for f64 {
    fn record(&self, w: &mut Writer<'_>) {
        w.f64(*self);
    }
    fn restore(&mut self, r: &mut Reader<'_>) -> Option<()> {
        *self = r.f64()?;
        Some(())
    }
    fn poison(&mut self, p: &mut Poison) {
        let v = f64::from(p.f32(0.0));
        *self = if v.to_bits() == self.to_bits() { v + 0.25 } else { v };
    }
}

impl Record for bool {
    fn record(&self, w: &mut Writer<'_>) {
        w.bool(*self);
    }
    fn restore(&mut self, r: &mut Reader<'_>) -> Option<()> {
        *self = r.bool()?;
        Some(())
    }
    fn poison(&mut self, p: &mut Poison) {
        let _ = p.step();
        *self = !*self;
    }
}

/// Unsigned integers (tag 1) and signed ones (tag 2), narrowed on restore.
macro_rules! record_int {
    ($t:ty, $w:ident, $r:ident) => {
        impl Record for $t {
            fn record(&self, w: &mut Writer<'_>) {
                w.$w(*self);
            }
            fn restore(&mut self, r: &mut Reader<'_>) -> Option<()> {
                *self = r.$r()?;
                Some(())
            }
            fn poison(&mut self, p: &mut Poison) {
                *self = self.wrapping_add(p.step() as $t);
            }
        }
    };
}

/// `u32`: scalars as tag 1, runs as a count, then tag-8 items of at most [`CHUNK`] values (one
/// word per value, through a stack buffer).
impl Record for u32 {
    fn record(&self, w: &mut Writer<'_>) {
        w.u32(*self);
    }
    fn restore(&mut self, r: &mut Reader<'_>) -> Option<()> {
        *self = r.u32()?;
        Some(())
    }
    fn record_run(items: &[Self], w: &mut Writer<'_>) {
        w.usize(items.len());
        for chunk in items.chunks(CHUNK) {
            w.uints(chunk.len(), chunk.iter().map(|&x| u64::from(x)));
        }
    }
    fn restore_run(items: &mut [Self], r: &mut Reader<'_>) -> Option<()> {
        (r.usize()? == items.len()).then_some(())?;
        let mut buf = [0u64; CHUNK];
        for chunk in items.chunks_mut(CHUNK) {
            let b = buf.get_mut(..chunk.len())?;
            r.u64s(b)?;
            for (d, s) in chunk.iter_mut().zip(b.iter()) {
                *d = u32::try_from(*s).ok()?;
            }
        }
        Some(())
    }
    fn poison(&mut self, p: &mut Poison) {
        *self = self.wrapping_add(u32::from(p.step()));
    }
}

record_int!(u64, u64, u64);
record_int!(usize, usize, usize);
record_int!(i32, i32, i32);
record_int!(i64, i64, i64);

/// Small integers: scalars as tag 1 or 2, runs packed little-endian into bytes.
macro_rules! record_small_int {
    ($t:ty, $w:ident, $r:ident) => {
        impl Record for $t {
            fn record(&self, w: &mut Writer<'_>) {
                w.$w(*self);
            }
            fn restore(&mut self, r: &mut Reader<'_>) -> Option<()> {
                *self = r.$r()?;
                Some(())
            }
            fn record_run(items: &[Self], w: &mut Writer<'_>) {
                write_packed(w, items, |x| x.to_le_bytes());
            }
            fn restore_run(items: &mut [Self], r: &mut Reader<'_>) -> Option<()> {
                read_packed(r, items, |x, b| *x = <$t>::from_le_bytes(b))
            }
            fn poison(&mut self, p: &mut Poison) {
                *self = self.wrapping_add(p.step() as $t);
            }
        }
    };
}

record_small_int!(u8, u8, u8);
record_small_int!(i8, i8, i8);
record_small_int!(u16, u16, u16);
record_small_int!(i16, i16, i16);

/// The dormant form of a run: a count, then each element's dormant form.
fn record_dormant_run<T: Record>(items: &[T], w: &mut Writer<'_>) {
    w.usize(items.len());
    items.iter().for_each(|x| x.record_dormant(w));
}

fn restore_dormant_run<T: Record>(items: &mut [T], r: &mut Reader<'_>) -> Option<()> {
    (r.usize()? == items.len()).then_some(())?;
    items.iter_mut().try_for_each(|x| x.restore_dormant(r))
}

impl<T: Record, const N: usize> Record for [T; N] {
    fn record(&self, w: &mut Writer<'_>) {
        T::record_run(self, w);
    }
    fn restore(&mut self, r: &mut Reader<'_>) -> Option<()> {
        T::restore_run(self, r)
    }
    fn record_dormant(&self, w: &mut Writer<'_>) {
        record_dormant_run(self, w);
    }
    fn restore_dormant(&mut self, r: &mut Reader<'_>) -> Option<()> {
        restore_dormant_run(self, r)
    }
    fn poison(&mut self, p: &mut Poison) {
        self.iter_mut().for_each(|x| x.poison(p));
    }
    fn poison_dormant(&mut self, p: &mut Poison) {
        self.iter_mut().for_each(|x| x.poison_dormant(p));
    }
}

/// A buffer the voice allocates at construction: its length is configuration, its elements state.
impl<T: Record> Record for Box<[T]> {
    fn record(&self, w: &mut Writer<'_>) {
        T::record_run(self, w);
    }
    fn restore(&mut self, r: &mut Reader<'_>) -> Option<()> {
        T::restore_run(self, r)
    }
    fn record_dormant(&self, w: &mut Writer<'_>) {
        record_dormant_run(self, w);
    }
    fn restore_dormant(&mut self, r: &mut Reader<'_>) -> Option<()> {
        restore_dormant_run(self, r)
    }
    fn poison(&mut self, p: &mut Poison) {
        self.iter_mut().for_each(|x| x.poison(p));
    }
    fn poison_dormant(&mut self, p: &mut Poison) {
        self.iter_mut().for_each(|x| x.poison_dormant(p));
    }
}

/// A presence bool, then the value when present (a receiving `None` takes `T::default()` first).
impl<T: Record + Default> Record for Option<T> {
    fn record(&self, w: &mut Writer<'_>) {
        w.present(self.as_ref(), |w, v| v.record(w));
    }
    fn restore(&mut self, r: &mut Reader<'_>) -> Option<()> {
        if r.bool()? {
            self.get_or_insert_with(T::default).restore(r)
        } else {
            *self = None;
            Some(())
        }
    }
    /// A present value is poisoned; an absent one becomes present (the presence changes).
    fn poison(&mut self, p: &mut Poison) {
        match self {
            Some(v) => v.poison(p),
            None => {
                let _ = p.step();
                *self = Some(T::default());
            }
        }
    }
}

/// Writes [`Record`] for a struct: one exhaustive destructuring of `Self { .. }` (no rest
/// pattern) in every function. Lists, all optional after the first:
/// - the first list: encoded, in order;
/// - `reset`: encoded, after the first list, except while the owning engine is dormant: the type's
///   reset (named at the call site) rewrites each of these fields in full, from constants and
///   other recorded state, before anything reads it (section 13's overwrite proof);
/// - `reset_via`: encoded, after `reset`; while the owning engine is dormant, each is written by
///   its own [`Record::record_dormant`], because the type's reset calls that field's reset;
/// - `config`, `unobservable`, `derived`: left out, each with a comment at the call site saying
///   why (configuration, a reference to immutable content, an overwrite-before-read or never-read
///   argument naming its witness, or the function restore re-derives it with).
///
/// The leading brackets hold the impl's generics (empty for none), and an optional `where [..]`
/// its bounds.
#[macro_export]
macro_rules! record_struct {
    ([$($g:tt)*] $ty:ty $(where [$($w:tt)*])? { $($f:ident),* $(,)? }
        $(reset { $($z:ident),* $(,)? })?
        $(reset_via { $($v:ident),* $(,)? })?
        $(config { $($c:ident),* $(,)? })?
        $(unobservable { $($u:ident),* $(,)? })?
        $(derived { $($d:ident),* $(,)? })?) => {
        impl<$($g)*> $crate::record::Record for $ty $(where $($w)*)? {
            fn record(&self, w: &mut $crate::record::Writer<'_>) {
                let Self { $($f,)* $($($z,)*)? $($($v,)*)? $($($c: _,)*)? $($($u: _,)*)? $($($d: _,)*)? } = self;
                $( $crate::record::Record::record($f, w); )*
                $($( $crate::record::Record::record($z, w); )*)?
                $($( $crate::record::Record::record($v, w); )*)?
            }
            fn restore(&mut self, r: &mut $crate::record::Reader<'_>) -> Option<()> {
                let Self { $($f,)* $($($z,)*)? $($($v,)*)? $($($c: _,)*)? $($($u: _,)*)? $($($d: _,)*)? } = self;
                $( $crate::record::Record::restore($f, r)?; )*
                $($( $crate::record::Record::restore($z, r)?; )*)?
                $($( $crate::record::Record::restore($v, r)?; )*)?
                Some(())
            }
            fn record_dormant(&self, w: &mut $crate::record::Writer<'_>) {
                let _ = &w;
                let Self { $($f,)* $($($z: _,)*)? $($($v,)*)? $($($c: _,)*)? $($($u: _,)*)? $($($d: _,)*)? } = self;
                $( $crate::record::Record::record($f, w); )*
                $($( $crate::record::Record::record_dormant($v, w); )*)?
            }
            fn restore_dormant(&mut self, r: &mut $crate::record::Reader<'_>) -> Option<()> {
                let _ = &r;
                let Self { $($f,)* $($($z: _,)*)? $($($v,)*)? $($($c: _,)*)? $($($u: _,)*)? $($($d: _,)*)? } = self;
                $( $crate::record::Record::restore($f, r)?; )*
                $($( $crate::record::Record::restore_dormant($v, r)?; )*)?
                Some(())
            }
            fn poison(&mut self, p: &mut $crate::record::Poison) {
                let _ = &p;
                let Self { $($f,)* $($($z,)*)? $($($v,)*)? $($($c: _,)*)? $($($u: _,)*)? $($($d: _,)*)? } = self;
                $( $crate::record::Record::poison($f, p); )*
                $($( $crate::record::Record::poison($z, p); )*)?
                $($( $crate::record::Record::poison($v, p); )*)?
            }
            fn poison_dormant(&mut self, p: &mut $crate::record::Poison) {
                let _ = &p;
                let Self { $($f: _,)* $($($z,)*)? $($($v,)*)? $($($c: _,)*)? $($($u: _,)*)? $($($d: _,)*)? } = self;
                $($( $crate::record::Record::poison($z, p); )*)?
                $($( $crate::record::Record::poison_dormant($v, p); )*)?
            }
        }
    };
}

/// Writes [`Record`] for an enum without payloads: its discriminant (tag 1), the variant's
/// position in the list, which must name every variant (an exhaustive match checks it).
#[macro_export]
macro_rules! record_enum {
    ($ty:ty { $($v:ident),+ $(,)? }) => {
        impl $crate::record::Record for $ty {
            fn record(&self, w: &mut $crate::record::Writer<'_>) {
                match self {
                    $(Self::$v)|+ => {}
                }
                let mut k = 0u8;
                let mut index = 0u8;
                $(
                    if matches!(self, Self::$v) {
                        index = k;
                    }
                    k += 1;
                )+
                let _ = k;
                w.u8(index);
            }
            fn restore(&mut self, r: &mut $crate::record::Reader<'_>) -> Option<()> {
                let index = r.u8()?;
                let mut k = 0u8;
                $(
                    if index == k {
                        *self = Self::$v;
                        return Some(());
                    }
                    k += 1;
                )+
                let _ = k;
                None
            }
            /// The next variant (the first after the last).
            fn poison(&mut self, p: &mut $crate::record::Poison) {
                let _ = p.step();
                let mut k = 0u8;
                let mut index = 0u8;
                $(
                    if matches!(self, Self::$v) {
                        index = k;
                    }
                    k += 1;
                )+
                let next = (index + 1) % k;
                k = 0;
                $(
                    if next == k {
                        *self = Self::$v;
                    }
                    k += 1;
                )+
                let _ = k;
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::Hasher;

    #[derive(Default)]
    struct Words(Vec<u64>);

    impl Hasher for Words {
        fn write(&mut self, _: &[u8]) {
            unreachable!("the record writer emits words only");
        }
        fn write_u64(&mut self, w: u64) {
            self.0.push(w);
        }
        fn finish(&self) -> u64 {
            0
        }
    }

    fn words(v: &impl Record) -> Vec<u64> {
        let mut sink = Words::default();
        v.record(&mut Writer::new(&mut sink));
        sink.0
    }

    /// Packed runs round-trip, a length that does not fit refuses, and a run spanning several
    /// chunks writes a count and then each chunk.
    #[test]
    fn packed_runs_round_trip_and_check_their_length() {
        let a: [i16; 300] = core::array::from_fn(|i| (i as i16).wrapping_mul(-311));
        let rec = words(&a);
        let mut count = Writer::counter();
        a.record(&mut count);
        assert_eq!(count.words(), rec.len() as u64, "the counting pass counts the words the writing pass emits");
        let mut b = [0i16; 300];
        let mut r = Reader::new(&rec);
        b.restore(&mut r).unwrap();
        assert!(r.finish());
        assert_eq!(a, b);
        let mut short = [0i16; 299];
        assert!(short.restore(&mut Reader::new(&rec)).is_none(), "another length refuses");
        let bytes: [u8; 600] = core::array::from_fn(|i| i as u8);
        let mut back = [0u8; 600];
        back.restore(&mut Reader::new(&words(&bytes))).unwrap();
        assert_eq!(bytes, back);
        let f: Box<[f32]> = vec![0.5, -0.25, 3.0].into_boxed_slice();
        let mut g: Box<[f32]> = vec![0.0; 3].into_boxed_slice();
        g.restore(&mut Reader::new(&words(&f))).unwrap();
        assert_eq!(f, g);
        let some: Option<u32> = Some(7);
        let mut none: Option<u32> = None;
        none.restore(&mut Reader::new(&words(&some))).unwrap();
        assert_eq!(none, Some(7));
    }
}
