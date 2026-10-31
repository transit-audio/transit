//! Versioned, fixed-shape continuation state. Readers fill already constructed storage;
//! lengths and allocation sizes never come from the file.

/// Internal checkpoint encoding shared with the host-rate adapter.
pub trait State {
    /// Append the little-endian representation to preallocated storage.
    fn write(&self, out: &mut Vec<u8>);
    /// Read into the existing shape, rejecting invalid values.
    fn read(&mut self, input: &mut &[u8]) -> Option<()>;
}

macro_rules! numbers {
    ($($t:ty),*) => {$ (impl State for $t {
        fn write(&self, out: &mut Vec<u8>) { out.extend_from_slice(&self.to_le_bytes()); }
        fn read(&mut self, input: &mut &[u8]) -> Option<()> {
            let n = std::mem::size_of::<Self>();
            *self = Self::from_le_bytes(input.get(..n)?.try_into().ok()?);
            *input = input.get(n..)?;
            Some(())
        }
    })*};
}
numbers!(u8, u16, i16, u32, i32, u64, i64, f32);

impl State for usize {
    fn write(&self, out: &mut Vec<u8>) {
        (*self as u64).write(out);
    }
    fn read(&mut self, input: &mut &[u8]) -> Option<()> {
        let mut n = 0u64;
        n.read(input)?;
        *self = usize::try_from(n).ok()?;
        Some(())
    }
}
impl State for bool {
    fn write(&self, out: &mut Vec<u8>) {
        u8::from(*self).write(out);
    }
    fn read(&mut self, input: &mut &[u8]) -> Option<()> {
        let mut n = 0u8;
        n.read(input)?;
        *self = match n {
            0 => false,
            1 => true,
            _ => return None,
        };
        Some(())
    }
}
impl<T: State> State for [T] {
    fn write(&self, out: &mut Vec<u8>) {
        for x in self {
            x.write(out);
        }
    }
    fn read(&mut self, input: &mut &[u8]) -> Option<()> {
        for x in self {
            x.read(input)?;
        }
        Some(())
    }
}
impl<T: State, const N: usize> State for [T; N] {
    fn write(&self, out: &mut Vec<u8>) {
        self.as_slice().write(out);
    }
    fn read(&mut self, input: &mut &[u8]) -> Option<()> {
        self.as_mut_slice().read(input)
    }
}
impl State for crate::float::F {
    fn write(&self, out: &mut Vec<u8>) {
        self.0.write(out);
    }
    fn read(&mut self, input: &mut &[u8]) -> Option<()> {
        self.0.read(input)?;
        self.0.is_finite().then_some(())
    }
}
impl State for crate::SeedButton {
    fn write(&self, out: &mut Vec<u8>) {
        let n: u8 = match self {
            Self::Off => 0,
            Self::On => 1,
            Self::Locked => 2,
        };
        n.write(out);
    }
    fn read(&mut self, input: &mut &[u8]) -> Option<()> {
        let mut n = 0u8;
        n.read(input)?;
        *self = match n {
            0 => Self::Off,
            1 => Self::On,
            2 => Self::Locked,
            _ => return None,
        };
        Some(())
    }
}

/// Encode fields in an explicit, versioned order, with a local invariant check on load.
#[macro_export]
macro_rules! state_fields {
    ($ty:ty, $s:ident, [$($f:ident),*], $valid:expr) => {
        impl $crate::checkpoint::State for $ty {
            fn write(&self, out: &mut Vec<u8>) {
                $(self.$f.write(out);)*
            }
            fn read(&mut self, input: &mut &[u8]) -> Option<()> {
                $(self.$f.read(input)?;)*
                let $s = self;
                ($valid).then_some(())
            }
        }
    };
}
