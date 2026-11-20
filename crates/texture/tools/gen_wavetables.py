#!/usr/bin/env python3
"""Regenerates src/wavetables.rs from the vendored resources file.

Run from the repository root:

    python crates/texture/tools/gen_wavetables.py

It parses the `wav_integrated_waves[]` array of
crates/macro-osc-oracle/vendor/beads/resources.cc (49920 integers), appends 260 zeros (one wave
plus its padding, the oracle's patches/all/0006) and writes the 50180 values, 20 to a line, as the
one table of src/wavetables.rs. That file is a data table (`DATA_TABLES` in xtask/src/file_size.rs
exempts it from the line budget; the gate checks it holds data only). The output is byte-identical
to the committed file; `git diff --exit-code crates/texture/src` after a run proves it. To move to
a new firmware pin: re-vendor, update the pin text in HEADER, run this, then the nulls.
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SOURCE = ROOT / "crates/macro-osc-oracle/vendor/beads/resources.cc"
OUT = ROOT / "crates/texture/src"
PAD = 260
PER_LINE = 20

HEADER = """//! The synth's wavetables (`wav_integrated_waves` in `beads/resources.cc`): 12 banks of 16 waves of
//! 256 samples plus 4 of padding for the Hermite reads, each wave pre-integrated (the voices
//! differentiate what they read), then one wave of zeros. Generated from `beads/resources.cc`
//! (eurorack `f4cfa769`, \u00c9milie Gillet, MIT; see LICENSE-THIRD-PARTY) by `tools/gen_wavetables.py`,
//! not edited. A data table: the whole table is one file, exempt from the line budget by
//! `DATA_TABLES` in `xtask/src/file_size.rs`.

#![forbid(unsafe_code)]

/// Samples per wave, `kTableSize`.
pub(crate) const TABLE_SIZE: usize = 256;

/// Waves per bank, `kNumWavesPerTable`.
pub(crate) const WAVES_PER_BANK: usize = 16;

/// Banks, `kNumBanks`.
pub(crate) const BANKS: usize = 12;

/// The stride between waves: a wave and its 4 samples of padding.
pub(crate) const WAVE_STRIDE: usize = TABLE_SIZE + 4;

/// The upstream table's length, `WAV_INTEGRATED_WAVES_SIZE`.
pub(crate) const UPSTREAM_LEN: usize = BANKS * WAVES_PER_BANK * WAVE_STRIDE;

const LEN: usize = UPSTREAM_LEN + WAVE_STRIDE;

/// The table and one more wave of zeros. A wave position of exactly 1.0 (the last wave of a
/// bank) interpolates toward the wave after it with a zero fraction; for the last bank that wave
/// is past the upstream table, where the firmware reads whatever follows it. The oracle's build
/// appends the same zeros (`patches/all/0006`), so both read the same values.
#[rustfmt::skip]
pub(crate) static WAVES: [i16; LEN] = [
"""


def waves():
    text = SOURCE.read_text(encoding="utf-8")
    start = text.index("const int16_t wav_integrated_waves[] = {")
    body = text[text.index("{", start) + 1 : text.index("};", start)]
    body = re.sub(r"//[^\n]*", "", body)
    return [int(t) for t in re.findall(r"-?\d+", body)]


def render(values):
    lines = [
        "    " + ", ".join(str(v) for v in values[i : i + PER_LINE]) + ","
        for i in range(0, len(values), PER_LINE)
    ]
    return HEADER + "\n".join(lines) + "\n];\n"


def main():
    values = waves()
    assert len(values) == 49920, len(values)
    values = values + [0] * PAD
    (OUT / "wavetables.rs").write_text(render(values), encoding="utf-8", newline="\n")
    print(f"{len(values)} values (with {PAD} zeros)")


if __name__ == "__main__":
    sys.exit(main())
