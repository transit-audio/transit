// C ABI around the texture processor's two other audio paths, for the transit
// oracle crate (ADR 0033, #241). Dev-only, built once (the module's buffer
// size does not concern either):
//
// - the synth (beads/dsp/synth.cc): what the module plays when nothing is
//   patched into its inputs, a wavetable oscillator and a granular player of
//   wavetable grains, then the limiters and the reverb, at 48 kHz;
// - the input stage (beads/gain_control.cc): the codec's raw 32-bit frames to
//   floats, with the automatic or manual gain, the soft clipping the tape
//   qualities select, and the mono normalization of an unpatched input.
//
// Every entry point is called from Rust only while the crate's process-wide
// session lock is held (src/ffi.rs).

#include <cstddef>
#include <cstdint>
#include <new>

#include "ftz_scope.h"
#include "beads/dsp/synth.h"
#include "beads/gain_control.h"

namespace {

using transit_oracle::FlushToZeroScope;

const size_t kMaxBlockSize = 24;
const size_t kReverbSize = 32768;

struct OracleTextureParameter {
  float base;
  float attenuverter;
  float cv;
  uint8_t has_cv;
  uint8_t reserved[3];
};

// The same layout as in texture_oracle.cc.
struct OracleTextureControls {
  OracleTextureParameter time;
  OracleTextureParameter size;
  OracleTextureParameter shape;
  OracleTextureParameter pitch;
  float density;
  float density_cv;
  float feedback;
  float dry_wet;
  float reverb;
  int32_t seed_button;
  uint8_t monophonic;
  uint8_t output_gate;
  uint8_t reserved[2];
};

// The synth and its reverb memory (on the module the reverb memory is the
// effect chain's, which the synth reuses while it plays; here it is its own),
// value-initialized as module globals are.
struct SynthHolder {
  beads::Synth synth;
  uint16_t reverb[kReverbSize];
};

// Value-initialized as a module global.
struct InputHolder {
  beads::GainControl gain_control;
};

void CopyParameter(const OracleTextureParameter& in, beads::RandomizedParameter* out) {
  out->base = in.base;
  out->attenuverter = in.attenuverter;
  out->cv = in.cv;
  out->has_cv = in.has_cv != 0;
}

}  // namespace

extern "C" {

// A synth Init'ed on zeroed reverb memory. NULL when the allocation fails.
void* oracle_texture_synth_new() {
  SynthHolder* h = new (std::nothrow) SynthHolder();
  if (h == NULL) {
    return NULL;
  }
  FlushToZeroScope fp;
  h->synth.Init(h->reverb);
  return h;
}

void oracle_texture_synth_free(void* synth) {
  delete static_cast<SynthHolder*>(synth);
}

// Synth::Render over `size` interleaved stereo float frames at `resolution`
// (0 to 3, the quality setting). `freeze` holds one flag per frame; `seed` one
// GateFlags byte per frame, or NULL. Returns 0 on success, -1 when size is 0
// or exceeds kMaxBlockSize, the resolution is outside 0 to 3, or the SEED
// button state outside 0 to 2.
int32_t oracle_texture_synth_render(
    void* synth,
    const OracleTextureControls* controls,
    int32_t resolution,
    const uint8_t* freeze,
    const uint8_t* seed,
    float* out,
    size_t size) {
  if (size == 0 || size > kMaxBlockSize || resolution < 0 || resolution > 3) {
    return -1;
  }
  if (controls->seed_button < beads::SEED_BUTTON_STATE_OFF ||
      controls->seed_button > beads::SEED_BUTTON_STATE_LOCKED) {
    return -1;
  }
  SynthHolder* h = static_cast<SynthHolder*>(synth);
  beads::Parameters p;
  CopyParameter(controls->time, &p.time);
  CopyParameter(controls->size, &p.size);
  CopyParameter(controls->shape, &p.shape);
  CopyParameter(controls->pitch, &p.pitch);
  p.monophonic_mode = controls->monophonic != 0;

  beads::ModulatedParameter density;
  density.base = controls->density;
  density.cv = controls->density_cv;

  beads::Processor::MixParameters m;
  m.feedback = controls->feedback;
  m.dry_wet = controls->dry_wet;
  m.reverb_amount = controls->reverb;
  m.output_gate = controls->output_gate != 0;

  bool freeze_flags[kMaxBlockSize];
  stmlib::GateFlags seed_flags[kMaxBlockSize];
  beads::FloatFrame output[kMaxBlockSize];
  for (size_t i = 0; i < size; ++i) {
    freeze_flags[i] = freeze[i] != 0;
    seed_flags[i] = seed ? seed[i] : 0;
  }
  {
    FlushToZeroScope fp;
    h->synth.Render(
        static_cast<beads::SeedButtonState>(controls->seed_button),
        seed ? seed_flags : NULL,
        freeze_flags,
        static_cast<beads::Synth::Resolution>(resolution),
        density, p, m,
        output, size);
  }
  for (size_t i = 0; i < size; ++i) {
    out[2 * i] = output[i].l;
    out[2 * i + 1] = output[i].r;
  }
  return 0;
}

// An input stage, value-initialized, then Init(sample_rate, mode, true) as the
// module does at power-up. NULL when the mode is outside 0 to 2 or the
// allocation fails.
void* oracle_texture_input_new(float sample_rate, int32_t mode) {
  if (mode < 0 || mode > 2) {
    return NULL;
  }
  InputHolder* h = new (std::nothrow) InputHolder();
  if (h == NULL) {
    return NULL;
  }
  FlushToZeroScope fp;
  h->gain_control.Init(
      sample_rate, static_cast<beads::GainControl::SoftClippingMode>(mode), true);
  return h;
}

void oracle_texture_input_free(void* input) {
  delete static_cast<InputHolder*>(input);
}

// GainControl::Init, as the module calls it on every codec restart (a quality
// change keeps the learned gain: reset false). Returns -1 for a mode outside
// 0 to 2.
int32_t oracle_texture_input_init(void* input, float sample_rate, int32_t mode, uint8_t reset) {
  if (mode < 0 || mode > 2) {
    return -1;
  }
  FlushToZeroScope fp;
  static_cast<InputHolder*>(input)->gain_control.Init(
      sample_rate,
      static_cast<beads::GainControl::SoftClippingMode>(mode),
      reset != 0);
  return 0;
}

// GainControl::Process over `size` interleaved raw 32-bit frames into float
// frames. Returns -1 when size is 0 or exceeds kMaxBlockSize.
int32_t oracle_texture_input_process(
    void* input,
    uint8_t active,
    uint8_t gain,
    uint8_t left_normalized,
    uint8_t right_normalized,
    const int32_t* in,
    float* out,
    size_t size) {
  if (size == 0 || size > kMaxBlockSize) {
    return -1;
  }
  beads::Codec::Frame frames[kMaxBlockSize];
  beads::FloatFrame output[kMaxBlockSize];
  for (size_t i = 0; i < size; ++i) {
    frames[i].l = in[2 * i];
    frames[i].r = in[2 * i + 1];
  }
  {
    FlushToZeroScope fp;
    static_cast<InputHolder*>(input)->gain_control.Process(
        active != 0, gain, left_normalized != 0, right_normalized != 0,
        frames, output, size);
  }
  for (size_t i = 0; i < size; ++i) {
    out[2 * i] = output[i].l;
    out[2 * i + 1] = output[i].r;
  }
  return 0;
}

}  // extern "C"
