// Force-included before every translation unit of the oracle build (build.rs), so the vendored
// sources under vendor/ compile unchanged on every CI compiler. Nothing here changes arithmetic.
#ifndef TRANSIT_ORACLE_COMPAT_H_
#define TRANSIT_ORACLE_COMPAT_H_

// M_PI and friends are behind this define in the MSVC and MinGW C runtimes (glibc and Apple
// expose them anyway).
#ifndef _USE_MATH_DEFINES
#define _USE_MATH_DEFINES
#endif

#ifdef _MSC_VER
// stmlib/utils/dsp.h marks helpers always_inline with a GCC attribute; MSVC has no such syntax.
#define __attribute__(x)
#endif

// The TEST branch of plaits/user_data.h calls printf in inline mock flash functions (never called
// by the oracle) without including <cstdio> itself.
#include <cstdio>
#include <cmath>

#endif  // TRANSIT_ORACLE_COMPAT_H_
