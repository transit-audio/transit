// C ABI around the vendored modal voice DSP (the eurorack `elements/dsp` part), for the transit
// oracle crate (ADR 0033, #198 slice 2). Dev-only: built by build.rs for tests and CI, never linked
// into the app.
//
// Every entry point is called from Rust only while the crate's process-wide session lock is held
// (src/ffi.rs), because stmlib::Random keeps its state in one static.

#include <cstddef>
#include <cstdint>
#include <new>

#include "ftz_scope.h"
#include "elements/dsp/dsp.h"
#include "elements/dsp/part.h"

namespace {

using transit_oracle::FlushToZeroScope;

// The reverb's sample memory, as the firmware's `reverb_buffer` (elements/elements.cc): 32768
// 16-bit words.
const size_t kReverbBufferSize = 32768;

// The fields of Patch (elements/dsp/patch.h) the module's CV scaler writes every block
// (elements/cv_scaler.cc). The other five (the side channel's modulation frequency and offset, the
// reverb's diffusion and low-pass, the exciter signature) are written by Init and Seed only.
struct OracleModalVoicePatch {
  float exciter_envelope_shape;
  float exciter_bow_level;
  float exciter_bow_timbre;
  float exciter_blow_level;
  float exciter_blow_meta;
  float exciter_blow_timbre;
  float exciter_strike_level;
  float exciter_strike_meta;
  float exciter_strike_timbre;
  float resonator_geometry;
  float resonator_brightness;
  float resonator_damping;
  float resonator_position;
  float space;
};

// Mirrors PerformanceState (elements/dsp/part.h) with fixed-width fields (the bool as uint8_t,
// after the floats).
struct OracleModalVoicePerformance {
  float note;
  float modulation;
  float strength;
  uint8_t gate;
};

// The part and its reverb memory. On the module both are globals in static storage, so every
// member Init leaves unwritten (the part's panic flag, the modal resonator's clock divider and
// LFO phase, the exciters' sample phases and particle range, the tube's delay line, the strings'
// resampler phase, the envelopes' unused fifth level, the easter-egg voice's smoothed damping and
// feedback and its FM phases) starts at zero. `new Holder()` value-initializes it: Holder has no
// user-provided constructor, so the whole object is zero-initialized before the members'
// constructors run (C++11 [dcl.init]/7), which reproduces that power-on state.
struct ModalVoiceHolder {
  uint16_t reverb_buffer[kReverbBufferSize];
  elements::Part part;
};

}  // namespace

extern "C" {

// The firmware's processing constants, so the Rust side can check its copies.
size_t oracle_modal_voice_max_block_size() { return elements::kMaxBlockSize; }
float oracle_modal_voice_sample_rate() { return elements::kSampleRate; }

// A part as the firmware's Init leaves it: the modal model, no easter egg, not bypassed, not
// seeded.
void* oracle_modal_voice_new() {
  ModalVoiceHolder* h = new (std::nothrow) ModalVoiceHolder();
  if (h == NULL) {
    return NULL;
  }
  FlushToZeroScope fp;
  h->part.Init(h->reverb_buffer);
  return h;
}

void oracle_modal_voice_free(void* part) {
  delete static_cast<ModalVoiceHolder*>(part);
}

// Part::Seed with three words, as the firmware seeds it with the chip's serial number
// (elements/elements.cc).
void oracle_modal_voice_seed(void* part, const uint32_t* words) {
  uint32_t copy[3] = { words[0], words[1], words[2] };
  FlushToZeroScope fp;
  static_cast<ModalVoiceHolder*>(part)->part.Seed(copy, 3);
}

// As the calibration menu sets it (elements/ui.cc). Returns -1 for a model outside 0..2 (nothing
// is changed), 0 otherwise.
int32_t oracle_modal_voice_set_model(void* part, int32_t model) {
  if (model < 0 || model > elements::RESONATOR_MODEL_STRINGS) {
    return -1;
  }
  static_cast<ModalVoiceHolder*>(part)->part.set_resonator_model(
      static_cast<elements::ResonatorModel>(model));
  return 0;
}

void oracle_modal_voice_set_easter_egg(void* part, uint8_t easter_egg) {
  static_cast<ModalVoiceHolder*>(part)->part.set_easter_egg(easter_egg != 0);
}

void oracle_modal_voice_set_bypass(void* part, uint8_t bypass) {
  static_cast<ModalVoiceHolder*>(part)->part.set_bypass(bypass != 0);
}

// Writes the panel fields of the patch, then Part::Process over `size` frames of the two audio
// inputs into main and aux (float, before the codec's int16 conversion). Returns 0 on success, -1
// when size is 0 or exceeds kMaxBlockSize (nothing is rendered: the part's scratch buffers hold
// kMaxBlockSize frames).
int32_t oracle_modal_voice_process(
    void* part,
    const OracleModalVoicePerformance* performance,
    const OracleModalVoicePatch* patch,
    const float* blow_in,
    const float* strike_in,
    float* main,
    float* aux,
    size_t size) {
  if (size == 0 || size > elements::kMaxBlockSize) {
    return -1;
  }
  elements::Part& p = static_cast<ModalVoiceHolder*>(part)->part;
  elements::Patch* q = p.mutable_patch();
  q->exciter_envelope_shape = patch->exciter_envelope_shape;
  q->exciter_bow_level = patch->exciter_bow_level;
  q->exciter_bow_timbre = patch->exciter_bow_timbre;
  q->exciter_blow_level = patch->exciter_blow_level;
  q->exciter_blow_meta = patch->exciter_blow_meta;
  q->exciter_blow_timbre = patch->exciter_blow_timbre;
  q->exciter_strike_level = patch->exciter_strike_level;
  q->exciter_strike_meta = patch->exciter_strike_meta;
  q->exciter_strike_timbre = patch->exciter_strike_timbre;
  q->resonator_geometry = patch->resonator_geometry;
  q->resonator_brightness = patch->resonator_brightness;
  q->resonator_damping = patch->resonator_damping;
  q->resonator_position = patch->resonator_position;
  q->space = patch->space;

  elements::PerformanceState s;
  s.gate = performance->gate != 0;
  s.note = performance->note;
  s.modulation = performance->modulation;
  s.strength = performance->strength;

  FlushToZeroScope fp;
  p.Process(s, blow_in, strike_in, main, aux, size);
  return 0;
}

}  // extern "C"
