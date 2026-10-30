// The flush-to-zero scope every oracle entry point opens around its calls into the vendored DSP
// (ADR 0033, #194). Shared by the macro-oscillator wrapper (oracle.cc) and the resonator wrapper
// (resonator_oracle.cc).
#ifndef TRANSIT_ORACLE_FTZ_SCOPE_H_
#define TRANSIT_ORACLE_FTZ_SCOPE_H_

#include <cstdint>

#if defined(__x86_64__) || defined(_M_X64) || defined(__i386__) || defined(_M_IX86)
#define TRANSIT_ORACLE_X86 1
#include <xmmintrin.h>
#endif

namespace transit_oracle {

// The firmware's desktop test harnesses turn flush-to-zero on (plaits/test/plaits_test.cc and
// rings/test/rings_test.cc). The mode is switched only around each call into the vendored code and
// restored before returning to Rust, which assumes the default floating-point environment
// everywhere it runs. On x86 this is MXCSR FTZ (denormal results flush; inputs are not zeroed, as
// upstream). On aarch64 it is FPCR.FZ, which also zeroes denormal inputs. Elsewhere the mode is
// left alone.
class FlushToZeroScope {
 public:
  FlushToZeroScope() {
#if defined(TRANSIT_ORACLE_X86)
    previous_ = _mm_getcsr();
    _MM_SET_FLUSH_ZERO_MODE(_MM_FLUSH_ZERO_ON);
#elif defined(__aarch64__) && (defined(__GNUC__) || defined(__clang__))
    __asm__ volatile("mrs %0, fpcr" : "=r"(previous_));
    const uint64_t next = previous_ | (UINT64_C(1) << 24);
    __asm__ volatile("msr fpcr, %0" : : "r"(next));
#else
    previous_ = 0;
#endif
  }
  ~FlushToZeroScope() {
#if defined(TRANSIT_ORACLE_X86)
    _mm_setcsr(static_cast<unsigned int>(previous_));
#elif defined(__aarch64__) && (defined(__GNUC__) || defined(__clang__))
    __asm__ volatile("msr fpcr, %0" : : "r"(previous_));
#endif
  }

 private:
  uint64_t previous_;
  FlushToZeroScope(const FlushToZeroScope&);
  void operator=(const FlushToZeroScope&);
};

}  // namespace transit_oracle

#endif  // TRANSIT_ORACLE_FTZ_SCOPE_H_
