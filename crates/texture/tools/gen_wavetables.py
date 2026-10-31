#!/usr/bin/env python3
"""Regenerates src/wavetables_a.rs and src/wavetables_b.rs from the vendored resources file.

Run from the repository root:

    python crates/texture/tools/gen_wavetables.py

It parses the `wav_integrated_waves[]` array of
crates/macro-osc-oracle/vendor/beads/resources.cc (49920 integers), keeps the first 24960 for the
first file, and writes the rest followed by 260 zeros (one wave plus its padding, the oracle's
patches/all/0006) to the second, 20 values to a line. The output is byte-identical to the
committed files; `git diff --exit-code crates/texture/src` after a run proves it. To move to a new
firmware pin: re-vendor, update the pin text in HEADER_A and HEADER_B, run this, then the nulls.
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SOURCE = ROOT / "crates/macro-osc-oracle/vendor/beads/resources.cc"
OUT = ROOT / "crates/texture/src"
HALF = 24960
PAD = 260
PER_LINE = 20

CREDIT = (
    "//! (eurorack `f4cfa769`, \u00c9milie Gillet, MIT; see LICENSE-THIRD-PARTY), split in two files for the\n"
    "//! line budget and joined in [`crate::wavetables`]. Generated, not edited.\n"
    "\n#![forbid(unsafe_code)]\n\n"
)
HEADER_A = (
    "//! `wav_integrated_waves`, banks 0 to 5: generated from `beads/resources.cc`\n" + CREDIT
    + f"/// The first half of the table.\n#[rustfmt::skip]\npub(crate) static WAVES_A: [i16; {HALF}] = [\n"
)
HEADER_B = (
    "//! `wav_integrated_waves`, banks 6 to 11, then the 260 zeros the oracle's build appends "
    "(`patches/all/0006`): generated from `beads/resources.cc`\n" + CREDIT
    + f"/// The second half of the table, then one wave of zeros.\n#[rustfmt::skip]\npub(crate) static WAVES_B: [i16; {HALF + PAD + 0}] = [\n"
)


def waves():
    text = SOURCE.read_text(encoding="utf-8")
    start = text.index("const int16_t wav_integrated_waves[] = {")
    body = text[text.index("{", start) + 1 : text.index("};", start)]
    body = re.sub(r"//[^\n]*", "", body)
    return [int(t) for t in re.findall(r"-?\d+", body)]


def render(header, values):
    lines = [
        "    " + ", ".join(str(v) for v in values[i : i + PER_LINE]) + ","
        for i in range(0, len(values), PER_LINE)
    ]
    return header + "\n".join(lines) + "\n];\n"


def main():
    values = waves()
    assert len(values) == 49920, len(values)
    first, second = values[:HALF], values[HALF:] + [0] * PAD
    header_b = HEADER_B.replace("SIZE_B", str(len(second)))
    (OUT / "wavetables_a.rs").write_text(render(HEADER_A, first), encoding="utf-8", newline="\n")
    (OUT / "wavetables_b.rs").write_text(render(header_b, second), encoding="utf-8", newline="\n")
    print(f"{len(values)} values: {len(first)} + {len(second)} (with {PAD} zeros)")


if __name__ == "__main__":
    sys.exit(main())
