// C ABI around the vendored texture processor's whole effect chain (the
// eurorack `beads/dsp` Processor: grain and delay triggers, the four buffer
// qualities, dry/wet, tape wow and flutter, the limiters and the reverb), for
// the transit oracle crate (ADR 0033, #241). Dev-only: built by build.rs for
// tests and CI, never linked into the app.
//
// build.rs compiles this file twice. The first build is the module's own
// buffer (768000 bytes: 4 seconds of 16-bit stereo at 48 kHz, the port's
// shortest); the second (TRANSIT_TEXTURE_LONG) overrides the buffer size and
// the bank size with the port's longest buffer and renames the vendored
// namespace (-Dbeads=...), so its symbols never meet the first build's. The
// exported names carry the variant (oracle_texture_* and
// oracle_texture_long_*).
//
// Every entry point is called from Rust only while the crate's process-wide
// session lock is held (src/ffi.rs), because stmlib::Random keeps its state in
// one static.
//
// The wrapper drives the processor as the firmware's desktop test harness does
// (beads/test/beads_test.cc): Init, Configure, then Process per block, with the
// parameters the module's CV reader would hand it.

#include <cstddef>
#include <cstdint>
#include <new>

#include "ftz_scope.h"
#include "beads/dsp/processor.h"

#ifdef TRANSIT_TEXTURE_LONG
#define TRANSIT_TEXTURE_SYMBOL(name) oracle_texture_long_##name
#else
#define TRANSIT_TEXTURE_SYMBOL(name) oracle_texture_##name
#endif

#ifndef TRANSIT_ORACLE_DELAY_BANK_SIZE
#define TRANSIT_ORACLE_DELAY_BANK_SIZE 262144
#endif

namespace {

using transit_oracle::FlushToZeroScope;

// The codec's largest block (kMaxCodecBlockSize in beads/drivers/codec.h, not
// vendored): the module's blocks are 6, 8 or 12 frames (its codec rate / 4000),
// and the MSVC build's fixed arrays (patches/msvc) hold 24.
const size_t kMaxBlockSize = 24;

// The module's three banks of delay memory (beads/beads.cc: one 512 KB block
// split in two and one 256 KB block), contiguous here.
const size_t kBankSize = TRANSIT_ORACLE_DELAY_BANK_SIZE;
const size_t kNumBanks = 3;

// The reverb's memory, 32768 words (beads/beads.cc).
const size_t kReverbSize = 32768;

// Mirrors RandomizedParameter (beads/dsp/randomized_parameter.h) with
// fixed-width fields.
struct OracleTextureParameter {
  float base;
  float attenuverter;
  float cv;
  uint8_t has_cv;
  uint8_t reserved[3];
};

// What the module's main loop hands Processor::Process each block
// (beads/beads.cc): the four randomized parameters, density and its CV, the
// mix parameters and the SEED button.
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

// The processor, its delay memory and its reverb memory. On the module all
// three are globals in static storage, so every member Init leaves unwritten
// starts at zero. `new Holder()` value-initializes the holder: it has no
// user-provided constructor, so the whole object is zero-initialized before
// the processor's (empty) constructor runs (C++11 [dcl.init]/7).
struct TextureHolder {
  beads::Processor processor;
  uint8_t* memory;
  uint16_t reverb[kReverbSize];
  // Nonzero: Process runs without the flush-to-zero scope (an evidence switch: the tests show the
  // float output tells the two apart, so the port's flush is checked bit for bit).
  uint8_t unflushed;
};

bool media_supported(int32_t media) {
  return media >= beads::Processor::MEDIA_DIRTY_TAPE &&
         media <= beads::Processor::MEDIA_DIGITAL;
}

void CopyParameter(const OracleTextureParameter& in, beads::RandomizedParameter* out) {
  out->base = in.base;
  out->attenuverter = in.attenuverter;
  out->cv = in.cv;
  out->has_cv = in.has_cv != 0;
}

}  // namespace

extern "C" {

size_t TRANSIT_TEXTURE_SYMBOL(max_block_size)() { return kMaxBlockSize; }

// kDelayBufferSizeBits: the delay memory every quality divides up.
size_t TRANSIT_TEXTURE_SYMBOL(delay_buffer_size_bits)() { return beads::kDelayBufferSizeBits; }

// A processor Init'ed on zeroed memory and Configure'd at `media` (0 dirty
// tape, 1 clean tape, 2 vintage digital, 3 digital) and `mono`, as the
// module's main loop leaves it before the first block. NULL when the media is
// outside 0 to 3 or an allocation fails.
void* TRANSIT_TEXTURE_SYMBOL(new)(int32_t media, uint8_t mono) {
  if (!media_supported(media)) {
    return NULL;
  }
  TextureHolder* h = new (std::nothrow) TextureHolder();
  if (h == NULL) {
    return NULL;
  }
  h->memory = new (std::nothrow) uint8_t[kBankSize * kNumBanks]();
  if (h->memory == NULL) {
    delete h;
    return NULL;
  }
  uint8_t* buffers[kNumBanks];
  for (size_t i = 0; i < kNumBanks; ++i) {
    buffers[i] = h->memory + i * kBankSize;
  }
  FlushToZeroScope fp;
  h->processor.Init(buffers, h->reverb);
  h->processor.Configure(static_cast<beads::Processor::Media>(media), mono != 0);
  return h;
}

void TRANSIT_TEXTURE_SYMBOL(free)(void* processor) {
  TextureHolder* h = static_cast<TextureHolder*>(processor);
  delete[] h->memory;
  delete h;
}

// As the module does when the quality setting or the mono normalization
// changes (MonitorStateChange in beads/beads.cc): the selected quality's
// buffer is cleared, the triggers, the delay controller and the limiters are
// re-initialized, the reverb keeps its memory. Returns -1 for a media outside
// 0 to 3 (nothing is changed), 0 otherwise.
int32_t TRANSIT_TEXTURE_SYMBOL(configure)(void* processor, int32_t media, uint8_t mono) {
  if (!media_supported(media)) {
    return -1;
  }
  FlushToZeroScope fp;
  static_cast<TextureHolder*>(processor)->processor.Configure(
      static_cast<beads::Processor::Media>(media), mono != 0);
  return 0;
}

// Whether Process runs inside the flush-to-zero scope (1, the default: the module's FPSCR.FZ) or
// in the default floating-point mode (0). For the tests that prove the flush matters.
void TRANSIT_TEXTURE_SYMBOL(set_flush)(void* processor, uint8_t flush) {
  static_cast<TextureHolder*>(processor)->unflushed = flush ? 0 : 1;
}

// The panel's MEDIA + FREEZE gesture: the recording's length drops to zero.
void TRANSIT_TEXTURE_SYMBOL(trim)(void* processor) {
  static_cast<TextureHolder*>(processor)->processor.Trim();
}

// Process over `size` interleaved stereo float frames. `freeze` holds one flag
// per frame; `seed` one GateFlags byte per frame, or NULL when nothing is
// patched into SEED. Returns 0 on success, -1 when size is 0, exceeds
// kMaxBlockSize, is odd at the clean-tape quality (its 2:1 converters consume
// frames in pairs and would leave the half-rate block unwritten), or the
// SEED button state is outside 0 to 2. Nothing is rendered then.
int32_t TRANSIT_TEXTURE_SYMBOL(process)(
    void* processor,
    const OracleTextureControls* controls,
    const uint8_t* freeze,
    const uint8_t* seed,
    const float* in,
    float* out,
    size_t size) {
  TextureHolder* h = static_cast<TextureHolder*>(processor);
  if (size == 0 || size > kMaxBlockSize) {
    return -1;
  }
  if (h->processor.ratio() != 1 && (size % h->processor.ratio()) != 0) {
    return -1;
  }
  if (controls->seed_button < beads::SEED_BUTTON_STATE_OFF ||
      controls->seed_button > beads::SEED_BUTTON_STATE_LOCKED) {
    return -1;
  }
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
  beads::FloatFrame input[kMaxBlockSize];
  beads::FloatFrame output[kMaxBlockSize];
  for (size_t i = 0; i < size; ++i) {
    freeze_flags[i] = freeze[i] != 0;
    seed_flags[i] = seed ? seed[i] : 0;
    input[i].l = in[2 * i];
    input[i].r = in[2 * i + 1];
  }
  if (h->unflushed) {
    h->processor.Process(
        static_cast<beads::SeedButtonState>(controls->seed_button),
        seed ? seed_flags : NULL,
        freeze_flags,
        density, p, m,
        input, output, size);
  } else {
    FlushToZeroScope fp;
    h->processor.Process(
        static_cast<beads::SeedButtonState>(controls->seed_button),
        seed ? seed_flags : NULL,
        freeze_flags,
        density, p, m,
        input, output, size);
  }
  for (size_t i = 0; i < size; ++i) {
    out[2 * i] = output[i].l;
    out[2 * i + 1] = output[i].r;
  }
  return 0;
}

}  // extern "C"
