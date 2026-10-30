// C ABI around the vendored resonator DSP (the eurorack `rings/dsp` part), for the transit oracle
// crate (ADR 0033, #198). Dev-only: built by build.rs for tests and CI, never linked into the app.
//
// Every entry point is called from Rust only while the crate's process-wide session lock is held
// (src/ffi.rs), because stmlib::Random keeps its state in one static.

#include <cstddef>
#include <cstdint>
#include <new>

#include "ftz_scope.h"
#include "rings/dsp/dsp.h"
#include "rings/dsp/part.h"

namespace {

using transit_oracle::FlushToZeroScope;

// The reverb's sample memory, as the firmware's `reverb_buffer` (rings/rings.cc): 32768 16-bit
// words, the size its FxEngine addresses.
const size_t kReverbBufferSize = 32768;

// Mirrors Patch (rings/dsp/patch.h).
struct OracleResonatorPatch {
  float structure;
  float brightness;
  float damping;
  float position;
};

// Mirrors PerformanceState (rings/dsp/performance_state.h) with fixed-width fields (bools as
// uint8_t, after the floats).
struct OraclePerformanceState {
  float tonic;
  float note;
  float fm;
  int32_t chord;
  uint8_t strum;
  uint8_t internal_exciter;
  uint8_t internal_strum;
  uint8_t internal_note;
};

// The part and its reverb memory. On the module both are globals in static storage, so every
// member the part's Init leaves unwritten (the voice rotation counter, the strings' resampler
// phase, the FM voices' feedback sample, the reverb's low-pass states) starts at zero. Allocating
// the holder with `new Holder()` value-initializes it: Holder has no user-provided constructor,
// so the whole object is zero-initialized before the members' constructors run (C++11
// [dcl.init]/7), which reproduces that power-on state.
struct ResonatorHolder {
  uint16_t reverb_buffer[kReverbBufferSize];
  rings::Part part;
};

}  // namespace

extern "C" {

// The firmware's processing constants, so the Rust side can check its copies.
size_t oracle_resonator_max_block_size() { return rings::kMaxBlockSize; }
int32_t oracle_resonator_max_polyphony() { return rings::kMaxPolyphony; }
int32_t oracle_resonator_model_count() { return rings::RESONATOR_MODEL_LAST; }

// A part as the firmware's Init leaves it: polyphony 1, the modal model, not bypassed.
void* oracle_resonator_new() {
  ResonatorHolder* h = new (std::nothrow) ResonatorHolder();
  if (h == NULL) {
    return NULL;
  }
  FlushToZeroScope fp;
  h->part.Init(h->reverb_buffer);
  return h;
}

void oracle_resonator_free(void* part) {
  delete static_cast<ResonatorHolder*>(part);
}

// As the panel sets them (rings/ui.cc). Returns -1 for a model outside 0..RESONATOR_MODEL_LAST or
// a polyphony outside 1..kMaxPolyphony (nothing is changed), 0 otherwise.
int32_t oracle_resonator_set_model(void* part, int32_t model) {
  if (model < 0 || model >= rings::RESONATOR_MODEL_LAST) {
    return -1;
  }
  static_cast<ResonatorHolder*>(part)->part.set_model(static_cast<rings::ResonatorModel>(model));
  return 0;
}

int32_t oracle_resonator_set_polyphony(void* part, int32_t polyphony) {
  if (polyphony < 1 || polyphony > rings::kMaxPolyphony) {
    return -1;
  }
  static_cast<ResonatorHolder*>(part)->part.set_polyphony(polyphony);
  return 0;
}

void oracle_resonator_set_bypass(void* part, uint8_t bypass) {
  static_cast<ResonatorHolder*>(part)->part.set_bypass(bypass != 0);
}

// Part::Process over `size` frames of input, into out and aux (float, before the codec's int16
// conversion). Returns 0 on success, -1 when size is 0 or exceeds kMaxBlockSize (nothing is
// rendered: the part's scratch buffers hold kMaxBlockSize frames), or when the chord is outside
// the chord table (0 to kNumChords - 1).
int32_t oracle_resonator_process(
    void* part,
    const OraclePerformanceState* performance,
    const OracleResonatorPatch* patch,
    const float* in,
    float* out,
    float* aux,
    size_t size) {
  if (size == 0 || size > rings::kMaxBlockSize) {
    return -1;
  }
  if (performance->chord < 0 || performance->chord >= rings::kNumChords) {
    return -1;
  }
  rings::PerformanceState s;
  s.strum = performance->strum != 0;
  s.internal_exciter = performance->internal_exciter != 0;
  s.internal_strum = performance->internal_strum != 0;
  s.internal_note = performance->internal_note != 0;
  s.tonic = performance->tonic;
  s.note = performance->note;
  s.fm = performance->fm;
  s.chord = performance->chord;

  rings::Patch p;
  p.structure = patch->structure;
  p.brightness = patch->brightness;
  p.damping = patch->damping;
  p.position = patch->position;

  FlushToZeroScope fp;
  static_cast<ResonatorHolder*>(part)->part.Process(s, p, in, out, aux, size);
  return 0;
}

}  // extern "C"
