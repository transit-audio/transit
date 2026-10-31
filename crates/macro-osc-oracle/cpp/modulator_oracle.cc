// C ABI around the vendored meta-modulator (the eurorack `warps/dsp`
// Modulator: two saturating input amplifiers, the internal oscillator, the
// six cross-modulation algorithms at 6x oversampling and the 20-band
// vocoder), for the transit oracle crate (ADR 0033, #200). Dev-only: built by
// build.rs for tests and CI, never linked into the app.
//
// Every entry point is called from Rust only while the crate's process-wide
// session lock is held (src/ffi.rs), because stmlib::Random (the internal
// oscillator's noise) keeps its state in one static.
//
// The wrapper drives the modulator as the firmware's main loop does
// (warps/warps.cc): Init(96000), then per codec block the parameters the
// module's CV scaler would hand it and Process. The module's codec runs at
// 96 kHz in blocks of 60 frames (codec.Start(60, ...)); the desktop test
// harness (warps/test/warps_test.cc) uses 96-frame blocks, which take the
// other branch of the 6x down converter. The oracle takes the module's
// blocks.

#include <cstddef>
#include <cstdint>
#include <new>

#include "ftz_scope.h"
#include "warps/dsp/modulator.h"

namespace {

using transit_oracle::FlushToZeroScope;

// The module's codec block, frames (warps/warps.cc).
const size_t kCodecBlockSize = 60;

// The module's sample rate (warps/warps.cc: modulator.Init(96000.0f)).
const float kSampleRate = 96000.0f;

// Mirrors the parameters of warps::Parameters (warps/dsp/parameters.h) the
// main processing path reads; the frequency shifter's are left at zero.
struct OracleModulatorParameters {
  float channel_drive[2];
  float modulation_algorithm;
  float modulation_parameter;
  float note;
  int32_t carrier_shape;
};

// The modulator. On the module it is a global in static storage, so every
// member Init leaves unwritten starts at zero: the input amplifiers' level
// followers and gains (level_, pre_gain_, post_gain_), which the first block
// reads. Allocating the holder with `new Holder()` value-initializes it:
// Holder has no user-provided constructor, so the whole object is
// zero-initialized before the members' constructors run (C++11
// [dcl.init]/7). build.rs passes -flifetime-dse=1 to GCC so that zeroing is
// kept in front of Modulator's empty constructor.
struct ModulatorHolder {
  warps::Modulator modulator;
  uint8_t flush;
};

}  // namespace

extern "C" {

// The firmware's processing constants, so the Rust side can check its copies.
size_t oracle_modulator_codec_block_size() { return kCodecBlockSize; }
size_t oracle_modulator_max_block_size() { return warps::kMaxBlockSize; }
size_t oracle_modulator_oversampling() { return warps::kOversampling; }

// A modulator as the firmware's Init leaves it, flushing to zero.
void* oracle_modulator_new() {
  ModulatorHolder* h = new (std::nothrow) ModulatorHolder();
  if (h == NULL) {
    return NULL;
  }
  h->flush = 1;
  FlushToZeroScope fp;
  h->modulator.Init(kSampleRate);
  return h;
}

void oracle_modulator_free(void* modulator) {
  delete static_cast<ModulatorHolder*>(modulator);
}

// Whether Process runs inside the flush-to-zero scope (the default, as every
// oracle of the family); 0 runs it in the default floating-point mode, an
// evidence switch for the test that asks whether the flush moves a sample.
void oracle_modulator_set_flush(void* modulator, uint8_t flush) {
  static_cast<ModulatorHolder*>(modulator)->flush = flush;
}

// Modulator::Process over `size` stereo int16 frames (left: input 1, the
// carrier; right: input 2, the modulator) into `out` (left: the main output;
// right: the aux output). Returns 0 on success, -1 when size is not a
// multiple of 12 between 12 and 60 (the vocoder's 3x and 4x decimators need
// whole groups of 12 frames, else they leave their bands unwritten, and above
// 63 frames the 6x down converter takes its other branch), or when a
// parameter is outside what the C++ can index its tables with (drives,
// algorithm and parameter within 0 to 1, the note within 0 to 127, the
// carrier shape within 0 to 3; every value finite).
int32_t oracle_modulator_process(
    void* modulator,
    const OracleModulatorParameters* parameters,
    const int16_t* in,
    int16_t* out,
    size_t size) {
  if (size == 0 || size > kCodecBlockSize || size % 12 != 0) {
    return -1;
  }
  const OracleModulatorParameters& p = *parameters;
  for (int i = 0; i < 2; ++i) {
    if (!(p.channel_drive[i] >= 0.0f && p.channel_drive[i] <= 1.0f)) {
      return -1;
    }
  }
  if (!(p.modulation_algorithm >= 0.0f && p.modulation_algorithm <= 1.0f) ||
      !(p.modulation_parameter >= 0.0f && p.modulation_parameter <= 1.0f) ||
      !(p.note >= 0.0f && p.note <= 127.0f) ||
      p.carrier_shape < 0 || p.carrier_shape > 3) {
    return -1;
  }
  ModulatorHolder* h = static_cast<ModulatorHolder*>(modulator);
  warps::Parameters* q = h->modulator.mutable_parameters();
  q->channel_drive[0] = p.channel_drive[0];
  q->channel_drive[1] = p.channel_drive[1];
  q->modulation_algorithm = p.modulation_algorithm;
  q->modulation_parameter = p.modulation_parameter;
  q->frequency_shift_pot = 0.0f;
  q->frequency_shift_cv = 0.0f;
  q->phase_shift = 0.0f;
  q->note = p.note;
  q->carrier_shape = p.carrier_shape;

  // warps::ShortFrame is two shorts, the layout of the codec's frames; the
  // firmware casts its codec buffers the same way. Copies keep the caller's
  // input untouched.
  warps::ShortFrame input[kCodecBlockSize];
  warps::ShortFrame output[kCodecBlockSize];
  for (size_t i = 0; i < size; ++i) {
    input[i].l = in[2 * i];
    input[i].r = in[2 * i + 1];
  }
  if (h->flush) {
    FlushToZeroScope fp;
    h->modulator.Process(input, output, size);
  } else {
    h->modulator.Process(input, output, size);
  }
  for (size_t i = 0; i < size; ++i) {
    out[2 * i] = output[i].l;
    out[2 * i + 1] = output[i].r;
  }
  return 0;
}

}  // extern "C"
