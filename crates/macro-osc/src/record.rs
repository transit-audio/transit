//! Test seam (feature `testing`; ADR 0029, amendment for #742, sections 12 and 13): the port's
//! complete state record and its restore, over the `transit-state-record` codec. Never on the
//! audio path, and compiled only with the feature (the crate's only workspace dependency, the
//! codec, is optional behind it).
//!
//! Every type the voice owns implements [`Record`] in its defining module, so private fields are
//! reachable: [`record_struct!`] writes both functions with one exhaustive destructuring of
//! `Self { .. }` and no rest pattern, so a new field does not compile until its treatment is
//! decided; [`record_enum!`] matches every variant. A field is either encoded, in declaration
//! order, or named in the macro's `config` list with a comment saying why: the voice's
//! configuration (its rate and what is computed from it at construction, which the record's
//! configuration child pins) or a reference to immutable content (a firmware table, a word bank),
//! which is the same in every instance. Nothing is derived: what an implementation could
//! recompute (a coefficient, a scale) is encoded, so restore never keeps the receiving
//! instance's own value of state. Scratch buffers are encoded too (no overwrite proofs needed).
//!
//! Runs: an `f32` run is one tag-6 item; `u8`, `i8`, `u16` and `i16` runs are packed little-endian
//! into tag-7 items of at most [`CHUNK`] bytes behind a count (integers, never floats, under the
//! byte tag); anything else is a count then each element. Restore checks every length against the
//! receiving instance's (lengths are configuration: the voice allocates nothing after
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
}

/// The largest tag-7 item a packed run writes (a stack buffer: nothing allocates).
pub const CHUNK: usize = 256;

/// Writes `items` packed `N` bytes each by `pack`: a count of bytes, then tag-7 items of whole
/// items, at most [`CHUNK`] bytes each (`N` is at most [`CHUNK`]).
pub fn write_packed<T, const N: usize>(w: &mut Writer<'_>, items: &[T], pack: impl Fn(&T) -> [u8; N]) {
    const { assert!(N > 0 && N <= CHUNK) };
    w.usize(items.len() * N);
    let mut buf = [0u8; CHUNK];
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
}

impl Record for f64 {
    fn record(&self, w: &mut Writer<'_>) {
        w.f64(*self);
    }
    fn restore(&mut self, r: &mut Reader<'_>) -> Option<()> {
        *self = r.f64()?;
        Some(())
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
        }
    };
}

record_int!(u32, u32, u32);
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
        }
    };
}

record_small_int!(u8, u8, u8);
record_small_int!(i8, i8, i8);
record_small_int!(u16, u16, u16);
record_small_int!(i16, i16, i16);

impl<T: Record, const N: usize> Record for [T; N] {
    fn record(&self, w: &mut Writer<'_>) {
        T::record_run(self, w);
    }
    fn restore(&mut self, r: &mut Reader<'_>) -> Option<()> {
        T::restore_run(self, r)
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
}

/// Writes [`Record`] for a struct: one exhaustive destructuring of `Self { .. }` (no rest
/// pattern) in both functions. The first list is encoded in order; the optional `config` list is
/// left out, each with a comment at the call site saying why (configuration, or a reference to
/// immutable content). The leading brackets hold the impl's generics (empty for none), and an
/// optional `where [..]` its bounds.
#[macro_export]
macro_rules! record_struct {
    ([$($g:tt)*] $ty:ty $(where [$($w:tt)*])? { $($f:ident),* $(,)? } $(config { $($c:ident),* $(,)? })?) => {
        impl<$($g)*> $crate::record::Record for $ty $(where $($w)*)? {
            fn record(&self, w: &mut $crate::record::Writer<'_>) {
                let Self { $($f,)* $($($c: _,)*)? } = self;
                $( $crate::record::Record::record($f, w); )*
            }
            fn restore(&mut self, r: &mut $crate::record::Reader<'_>) -> Option<()> {
                let Self { $($f,)* $($($c: _,)*)? } = self;
                $( $crate::record::Record::restore($f, r)?; )*
                Some(())
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
