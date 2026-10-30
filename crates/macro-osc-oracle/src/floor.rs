//! The residual floor of the oracle, per CI runner family.
//!
//! The floor is the residual of the oracle against itself: the same sweep rendered twice with the
//! same seed, in one process and in a second process (`tests/self_floor.rs`). It was measured as
//! bit-identical (`-inf` dBFS, zero differing samples) on Windows x86-64 (MSVC) on 2026-10-29, and for
//! the resonator (`tests/resonator_self_floor.rs`, #198) and the granular processor
//! (`tests/granular_self_floor.rs`, #199) on 2026-10-30; Linux
//! and macOS are proved by CI, whose workspace test runs `tests/self_floor.rs` on every run.
//! A gate on the port (#195) starts from an exact reference: whatever residual the port shows is
//! the port's.
//!
//! Each family compiles the C++ with its own compiler (MSVC, GCC, Apple Clang) and runs on its
//! own instruction set, so the oracle's output is compared only within a family, never across:
//! the port and the oracle are always rendered on the same runner. The procedure to re-measure is
//! in `docs/contributing/evidence/gates.md` ("Oracle null tests") and the crate README.

#![forbid(unsafe_code)]

/// A CI runner family: one compiler and instruction set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunnerFamily {
    /// `windows-latest`: MSVC, x86-64.
    WindowsMsvcX86_64,
    /// `ubuntu-22.04`: GCC, x86-64.
    LinuxGnuX86_64,
    /// `macos-latest`: Apple Clang, AArch64.
    MacosAarch64,
    /// Any other target: no floor has been measured.
    Unmeasured,
}

impl RunnerFamily {
    /// The family of the target this crate was compiled for.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(all(target_os = "windows", target_env = "msvc", target_arch = "x86_64")) {
            Self::WindowsMsvcX86_64
        } else if cfg!(all(target_os = "linux", target_env = "gnu", target_arch = "x86_64")) {
            Self::LinuxGnuX86_64
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            Self::MacosAarch64
        } else {
            Self::Unmeasured
        }
    }

    /// The oracle's self-comparison floor on this family, peak and RMS, dBFS: `-inf` (bit-identical),
    /// measured on Windows x86-64 and proved on Linux and macOS by CI; `None` for any other
    /// target.
    #[must_use]
    pub const fn self_floor_dbfs(self) -> Option<f64> {
        match self {
            Self::WindowsMsvcX86_64 | Self::LinuxGnuX86_64 | Self::MacosAarch64 => Some(f64::NEG_INFINITY),
            Self::Unmeasured => None,
        }
    }
}
