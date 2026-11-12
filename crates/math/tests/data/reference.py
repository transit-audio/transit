"""Arbitrary-precision reference values for transit-math (ADR 0055 2.3 and 2.9).

Pure Python `decimal` and `fractions`, no third-party package. From this folder, run:

    python -I reference.py <outdir>

It regenerates gain_grid.txt, pan_grid.txt and f64_reference.txt byte for byte (point <outdir> at
this folder to overwrite them, or elsewhere and diff). Any change to the output is a change to a
pinned reference and needs its hashes in tests/gain.rs updated.

Writes:
  gain_grid.txt     correctly rounded f32 of 10^(x/20) for x = f32(k/100), k = -14400..=4800
  pan_grid.txt      correctly rounded f32 of the pan law at the panel's positions
  f64_reference.txt correctly rounded f64 (and the residual in ulps) of each f64 function
and prints the decade bits, the margins and the constants.
"""

import math
import struct
import sys
from decimal import Decimal as D, getcontext, localcontext
from fractions import Fraction as F

getcontext().prec = 90

# ---------------------------------------------------------------- exact rounding


def frac_of(x):
    """Exact Fraction of a Decimal or float."""
    if isinstance(x, F):
        return x
    if isinstance(x, float):
        return F(x)
    return F(x)  # Decimal -> exact Fraction


def round_binary(v, mant_bits, emin):
    """Round a positive Fraction to the nearest binary float with `mant_bits` significand bits
    (ties to even), minimum normal exponent `emin`. Returns (significand int, exponent of its ulp),
    and the distance of v to the nearest rounding midpoint in ulps."""
    assert v > 0
    # floor(log2 v)
    e = v.numerator.bit_length() - v.denominator.bit_length()
    if F(2) ** e > v:
        e -= 1
    while F(2) ** (e + 1) <= v:
        e += 1
    ue = max(e, emin) - (mant_bits - 1)
    q = v / (F(2) ** ue)
    n = q.numerator // q.denominator
    rem = q - n
    if rem > F(1, 2) or (rem == F(1, 2) and n % 2 == 1):
        n += 1
    # distance to the nearest midpoint (midpoints at integer + 1/2 in units of ulp)
    frac_part = q - (q.numerator // q.denominator)
    margin = abs(frac_part - F(1, 2))
    return n, ue, margin


def to_f32_bits(v):
    v = frac_of(v)
    if v == 0:
        return 0, F(1, 2)
    sign = 0
    if v < 0:
        sign, v = 1, -v
    n, ue, margin = round_binary(v, 24, -126)
    val = F(n) * F(2) ** ue
    bits = struct.unpack("<I", struct.pack("<f", float(val)))[0]  # val is exactly representable
    return bits | (sign << 31), margin


def to_f64_rounded(v):
    """(bits, residual in ulps of the rounded value, margin)."""
    v = frac_of(v)
    sign = 1
    if v < 0:
        sign, v = -1, -v
    n, ue, margin = round_binary(v, 53, -1022)
    val = F(n) * F(2) ** ue
    resid = (v - val) / (F(2) ** ue)
    f = float(val) * sign
    bits = struct.unpack("<Q", struct.pack("<d", f))[0]
    return bits, float(resid) * sign, margin


def f32_from_bits(b):
    return struct.unpack("<f", struct.pack("<I", b))[0]


def f64_from_bits(b):
    return struct.unpack("<d", struct.pack("<Q", b))[0]


def f64_bits(x):
    return struct.unpack("<Q", struct.pack("<d", x))[0]


def f32_round_frac(fr):
    bits, _ = to_f32_bits(fr)
    return f32_from_bits(bits)


# ---------------------------------------------------------------- high-precision functions

with localcontext() as c:
    c.prec = 200
    # Machin: pi = 16 atan(1/5) - 4 atan(1/239)

    def _atan_inv(n):
        x = D(1) / n
        x2 = x * x
        s, t, k = D(0), x, 1
        while True:
            term = t / k
            if abs(term) < D(10) ** -195:
                break
            s += term if (k // 2) % 2 == 0 else -term
            t *= x2
            k += 2
        return s

    PI = +(16 * _atan_inv(5) - 4 * _atan_inv(239))
    LN2 = D(2).ln()
    LN10 = D(10).ln()


def d(x):
    """Exact Decimal of a float."""
    return D(x)


def hp_exp(x):
    return d(x).exp() if not isinstance(x, D) else x.exp()


def hp_sin_cos(x):
    with localcontext() as c:
        c.prec = 200
        x = D(x) if not isinstance(x, D) else x
        two_pi = 2 * PI
        k = (x / two_pi).to_integral_value()
        r = x - k * two_pi
        # Taylor on |r| <= pi (precision 200 absorbs the growth of the terms)
        s, cc = D(0), D(0)
        term = r
        n = 1
        while abs(term) > D(10) ** -190:
            s += term
            term = -term * r * r / ((n + 1) * (n + 2))
            n += 2
        term = D(1)
        n = 0
        while abs(term) > D(10) ** -190:
            cc += term
            term = -term * r * r / ((n + 1) * (n + 2))
            n += 2
        return +s, +cc


def hp_atan(t):
    with localcontext() as c:
        c.prec = 200
        t = D(t) if not isinstance(t, D) else t
        neg = t < 0
        t = abs(t)
        inv = t > 1
        if inv:
            t = 1 / t
        halvings = 0
        while t > D("0.01"):
            t = t / (1 + (1 + t * t).sqrt())
            halvings += 1
        s, term, k, t2 = D(0), t, 1, t * t
        p = t
        while True:
            term = p / k
            if abs(term) < D(10) ** -190:
                break
            s += term if (k // 2) % 2 == 0 else -term
            p *= t2
            k += 2
        a = s * (2 ** halvings)
        if inv:
            a = PI / 2 - a
        return -a if neg else a


def hp_atan2(y, x):
    with localcontext() as c:
        c.prec = 200
        y, x = D(y), D(x)
        if x > 0:
            return hp_atan(y / x)
        if x < 0:
            a = hp_atan(y / x)
            return a + PI if y >= 0 else a - PI
        return PI / 2 if y > 0 else -PI / 2


def hp(name, a, b=None):
    with localcontext() as c:
        c.prec = 120
        x = D(a)
        if name == "sin":
            return hp_sin_cos(x)[0]
        if name == "cos":
            return hp_sin_cos(x)[1]
        if name == "tan":
            s, co = hp_sin_cos(x)
            return s / co
        if name == "exp":
            return x.exp()
        if name == "exp2":
            return (x * LN2).exp()
        if name == "ln":
            return x.ln()
        if name == "log2":
            return x.ln() / LN2
        if name == "log10":
            return x.log10()
        if name == "pow":
            return (D(b) * x.ln()).exp()
        if name == "atan2":
            return hp_atan2(a, b)
    raise ValueError(name)


# ---------------------------------------------------------------- tables


def gain_exact(x):
    """10^(x/20) for the exact value of the float x."""
    with localcontext() as c:
        c.prec = 90
        return (D(x) / 20 * LN10).exp()


def write_gain_grid(out):
    vals = []
    worst = (1, None)
    for k in range(-14400, 4801):
        x = f32_round_frac(F(k, 100))
        bits, margin = to_f32_bits(gain_exact(x))
        vals.append(bits)
        if margin < worst[0]:
            worst = (margin, k)
    worst10 = (1, None)
    for k in range(-1440, 481):
        x = f32_round_frac(F(k, 10))
        bits, margin = to_f32_bits(gain_exact(x))
        assert bits == vals[k * 10 + 14400], k  # the 0.1 dB grid is a subset of the 0.01 dB grid
        if margin < worst10[0]:
            worst10 = (margin, k)
    with open(f"{out}/gain_grid.txt", "w", newline="\n") as fh:
        fh.write("# Correctly rounded f32 bits of 10^(x/20), x = the f32 nearest k/100, k = -14400..=4800 (-144 to +48 dB\n")
        fh.write("# in 0.01 dB steps; the 0.1 dB grid is every tenth entry). Generated once, offline, with an\n")
        fh.write("# arbitrary-precision script (Python decimal at 90 digits: exp(x/20 * ln 10) on the exact value of x,\n")
        fh.write("# rounded to nearest-even f32 in exact rational arithmetic). The script is reference.py, beside this file.\n")
        fh.write("# 16 values per line, hexadecimal, in k order.\n")
        for i in range(0, len(vals), 16):
            fh.write(" ".join(f"{v:08x}" for v in vals[i:i + 16]) + "\n")
    print(f"gain grid 0.01: {len(vals)} values, closest to a midpoint {float(worst[0]):.3e} ulp at k={worst[1]}")
    print(f"gain grid 0.1 : closest {float(worst10[0]):.3e} ulp at k={worst10[1]}")
    return vals


def decades():
    print("decades (dB, bits, value, margin):")
    for db in range(-140, 201, 20):
        bits, margin = to_f32_bits(gain_exact(float(db)))
        print(f"  {db:5d} 0x{bits:08x} {f32_from_bits(bits)!r} {float(margin):.3f}")


def pan_exact(p, center_db):
    """(l, r) of the power pan law for the exact float p, exponent center_db / -3.0103."""
    with localcontext() as c:
        c.prec = 120
        theta = (D(p) + 1) * PI / 4
        s, co = hp_sin_cos(theta)
        e = D(repr(center_db)) / D("-3.0103")
        l = (e * co.ln()).exp()
        r = (e * s.ln()).exp()
        return l, r


def write_pan_grid(out):
    lines = []
    worst = (1, None)
    for law in (-3.0, -4.5, -6.0):
        row = []
        for k in range(-99, 100):
            p = f32_round_frac(F(k, 100))
            l, r = pan_exact(p, law)
            lb, lm = to_f32_bits(l)
            rb, rm = to_f32_bits(r)
            row += [lb, rb]
            for m in (lm, rm):
                if m < worst[0]:
                    worst = (m, (law, k))
        lines.append((law, row))
    with open(f"{out}/pan_grid.txt", "w", newline="\n") as fh:
        fh.write("# Correctly rounded f32 bits of the pan law: l = cos(t)^e, r = sin(t)^e, t = (p + 1) * pi / 4,\n")
        fh.write("# e = center_db / -3.0103, p = the f32 nearest k/100 for k = -99..=99 (the panel's positions; the\n")
        fh.write("# hard ends are exact branches). One block per law (center -3, -4.5, -6 dB), pairs l r in k order.\n")
        fh.write("# Generated once, offline, with an arbitrary-precision script (Python decimal, pi to 200 digits, Taylor\n")
        fh.write("# series, nearest-even rounding in exact rational arithmetic). The script is reference.py, beside this file.\n")
        for law, row in lines:
            fh.write(f"law {law}\n")
            for i in range(0, len(row), 16):
                fh.write(" ".join(f"{v:08x}" for v in row[i:i + 16]) + "\n")
    print(f"pan grid: closest to a midpoint {float(worst[0]):.3e} ulp at {worst[1]}")


def lin(lo, hi, n):
    return [lo + (hi - lo) * i / (n - 1) for i in range(n)]


def geo(lo, hi, n):
    return [math.exp(math.log(lo) + (math.log(hi) - math.log(lo)) * i / (n - 1)) for i in range(n)]


def write_f64_reference(out):
    sets = {
        "sin": lin(-100.0, 100.0, 224) + geo(100.5, 1e6, 32),
        "cos": lin(-100.0, 100.0, 224) + geo(100.5, 1e6, 32),
        "tan": lin(-100.0, 100.0, 224) + geo(100.5, 1e6, 32),
        "exp": lin(-700.0, 700.0, 256),
        "exp2": lin(-1000.0, 1000.0, 256),
        "ln": geo(1e-300, 1e300, 256),
        "log2": geo(1e-300, 1e300, 256),
        "log10": geo(1e-300, 1e300, 256),
    }
    two = {
        "pow": [(x, y) for x in geo(1e-3, 1e3, 16) for y in lin(-8.0, 8.0, 16)],
        "atan2": [(y, x) for y in lin(-10.0, 10.0, 16) for x in lin(-10.0, 10.0, 16)],
    }
    worst = (1, None)
    with open(f"{out}/f64_reference.txt", "w", newline="\n") as fh:
        fh.write("# Reference values for the f64 functions of transit-math. Each line: name, argument bits (two for pow\n")
        fh.write("# and atan2: x y for pow, y x for atan2), the correctly rounded f64 bits of the exact result, and the\n")
        fh.write("# exact result minus that value in its ulps. Generated once, offline, with an arbitrary-precision script\n")
        fh.write("# (Python decimal at 120 to 200 digits: exp and ln from decimal, sin and cos by Taylor series after\n")
        fh.write("# reduction with pi to 200 digits, atan by argument halving and series; nearest-even rounding in exact\n")
        fh.write("# rational arithmetic). The script is reference.py, beside this file.\n")
        for name, args in sets.items():
            for a in args:
                bits, resid, margin = to_f64_rounded(hp(name, a))
                fh.write(f"{name} {f64_bits(a):016x} {bits:016x} {resid:+.6f}\n")
        for name, args in two.items():
            for a, b in args:
                bits, resid, margin = to_f64_rounded(hp(name, a, b))
                fh.write(f"{name} {f64_bits(a):016x} {f64_bits(b):016x} {bits:016x} {resid:+.6f}\n")
    print("f64 reference written")


def constants():
    with localcontext() as c:
        c.prec = 90
        v = {
            "LOG2_TO_DB": 20 * LN2 / LN10,
            "DB_TO_LOG2": LN10 / LN2 / 20,
            "LOG2_E": 1 / LN2,
            "LOG2_10": LN10 / LN2,
        }
    for k, x in v.items():
        b64, _, m64 = to_f64_rounded(x)
        b32, m32 = to_f32_bits(x)
        f32v = f32_from_bits(b32)
        # shortest decimal that rounds (nearest-even, exactly) to the f32
        short = None
        for digits in range(1, 12):
            s = f"{f32v:.{digits}g}"
            if to_f32_bits(F(D(s)))[0] == b32:
                short = s
                break
        print(f"{k}: f64 {f64_from_bits(b64)!r} (0x{b64:016x}), f32 0x{b32:08x} shortest {short}")


if __name__ == "__main__":
    out = sys.argv[1]
    constants()
    decades()
    write_gain_grid(out)
    write_pan_grid(out)
    write_f64_reference(out)
