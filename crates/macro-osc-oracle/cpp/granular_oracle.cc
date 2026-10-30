// C ABI around the vendored granular texture processor (the eurorack `clouds/dsp` processor), for
// the transit oracle crate (ADR 0033, #199). Dev-only: built by build.rs for tests and CI, never
// linked into the app.
//
// Every entry point is called from Rust only while the crate's process-wide session lock is held
// (src/ffi.rs), because stmlib::Random keeps its state in one static.
//
// The wrapper drives the processor as the firmware's desktop test harness does
// (clouds/test/clouds_test.cc): Init, the quality and the playback mode, Prepare; then, per block,
// the parameters, Process and Prepare. Two playback modes are wired: the granular mode (ported in
// #199 slice 1) and the pitch-shifter / time-stretcher mode (PLAYBACK_MODE_STRETCH, the WSOLA
// player, its correlator and the filters; the oracle for slice 2). The stretcher's correlator
// advances in Prepare, which the module runs from its main loop as often as the CPU allows and the
// harness runs once per block: the oracle follows the harness, so the reference is defined.

#include <cstddef>
#include <cstdint>
#include <new>

#include "ftz_scope.h"
#include "clouds/dsp/frame.h"
#include "clouds/dsp/granular_processor.h"

namespace {

using transit_oracle::FlushToZeroScope;

// The module's two memory blocks (clouds/clouds.cc): `block_mem` in main RAM and `block_ccm` in
// the core-coupled RAM, 128 bytes short of 64 KB.
const size_t kFirmwareLargeBufferSize = 118784;
const size_t kFirmwareSmallBufferSize = 65536 - 128;

// The FX workspace Prepare carves out of the memory that is not sample memory: the diffuser's 2048
// floats, the reverb's 16384 words, then the correlator's three blocks of (kMaxWSOLASize / 32 + 2)
// words, which the pitch shifter reuses as its own 4096-word FxEngine memory and clears in full
// (granular_processor.cc, fx/pitch_shifter.h): 8192 bytes from the correlator's start, more than
// the correlator's 1560. The sizes the wrapper accepts must leave room for all of it in both
// channel layouts: the BufferAllocator would hand out NULL for the first three, and the pitch
// shifter's clear would write past the block.
const size_t kCorrelatorSize = ((clouds::kMaxWSOLASize / 32) + 2) * 3 * sizeof(uint32_t);
const size_t kPitchShifterSize = 4096 * sizeof(uint16_t);
const size_t kWorkspaceSize =
    2048 * sizeof(float) + 16384 * sizeof(uint16_t) +
    (kPitchShifterSize > kCorrelatorSize ? kPitchShifterSize : kCorrelatorSize);

// The longest grain, in samples: the last entry of lut_grain_size (1024 * 2^4). A grain's start
// stays inside the recording buffer only while the buffer holds more samples than this
// (GranularSamplePlayer::ScheduleGrain computes `start` from `available`, which turns negative for
// grains longer than half the buffer).
const size_t kLongestGrain = 16384;

// The largest block the wrapper accepts. A grain's read position is 16.16 fixed point in an
// int32_t (Grain::OverlapAdd): a transposed grain reads up to a quarter of the recording B ahead,
// so its phase reaches about B * 16384, which overflows (undefined; it wraps negative and reads
// before the buffer) once B reaches 131072 samples. Every layout records at most `large` samples
// less the 8-sample tail (8-bit mono; `small` is smaller), so this one bound keeps B * 16384 below
// 2^31 in all of them.
const size_t kMaxLargeBufferSize = 131072;

// Mirrors Parameters (clouds/dsp/parameters.h), the fields the CV scaler writes, with fixed-width
// fields (bools as uint8_t, after the floats).
struct OracleGranularParameters {
  float position;
  float size;
  float pitch;
  float density;
  float texture;
  float dry_wet;
  float stereo_spread;
  float feedback;
  float reverb;
  uint8_t freeze;
  uint8_t trigger;
  uint8_t gate;
  uint8_t reserved;
};

// The processor and its two memory blocks. On the module all three are globals in static storage,
// so every member Init leaves unwritten (the freeze low-pass, the silence flag, the grain player's
// rate phasor, the reverb's low-pass states, the grains' fields) starts at zero. `new Holder()`
// value-initializes the holder: it has no user-provided constructor, so the whole object is
// zero-initialized before the processor's (empty) constructor runs (C++11 [dcl.init]/7). The
// memory blocks are value-initialized arrays, zeroed too.
struct GranularHolder {
  clouds::GranularProcessor processor;
  uint8_t* large;
  uint8_t* small;
};

// The playback modes the wrapper accepts: 0 granular, 1 stretch.
bool mode_supported(int32_t mode) {
  return mode == clouds::PLAYBACK_MODE_GRANULAR || mode == clouds::PLAYBACK_MODE_STRETCH;
}

bool sizes_supported(size_t large, size_t small) {
  // Stereo: each channel records into `small` bytes and the workspace is `large - small`. Mono:
  // the one channel records into `large` and the workspace is all of `small`. Every layout's
  // recording buffer (at most 2 bytes per sample, less the 8-sample interpolation tail) must hold
  // more than the longest grain, and none may reach the grain phase's limit (kMaxLargeBufferSize).
  return small >= kWorkspaceSize && large >= small + kWorkspaceSize && large <= kMaxLargeBufferSize &&
         small / 2 > kLongestGrain + 8 && small % 4 == 0 && large % 4 == 0;
}

}  // namespace

extern "C" {

// The firmware's processing constants, so the Rust side can check its copies.
size_t oracle_granular_max_block_size() { return clouds::kMaxBlockSize; }
size_t oracle_granular_firmware_large_size() { return kFirmwareLargeBufferSize; }
size_t oracle_granular_firmware_small_size() { return kFirmwareSmallBufferSize; }
size_t oracle_granular_workspace_size() { return kWorkspaceSize; }

// A processor in playback mode `mode` (0 granular, 1 stretch) at `quality` (0 to 3: bit 0 mono,
// bit 1 the 8-bit mu-law buffers at half the rate), Init'ed on two zeroed blocks of `large` and
// `small` bytes and Prepare'd, as the harness leaves it before its first block. NULL when the sizes
// leave no room for the workspace or the longest grain (see sizes_supported), when the quality is
// outside 0 to 3 or the mode outside 0 to 1, or when an allocation fails.
void* oracle_granular_new(size_t large, size_t small, int32_t quality, int32_t mode) {
  if (!sizes_supported(large, small) || quality < 0 || quality > 3 || !mode_supported(mode)) {
    return NULL;
  }
  GranularHolder* h = new (std::nothrow) GranularHolder();
  if (h == NULL) {
    return NULL;
  }
  h->large = new (std::nothrow) uint8_t[large]();
  h->small = new (std::nothrow) uint8_t[small]();
  if (h->large == NULL || h->small == NULL) {
    delete[] h->large;
    delete[] h->small;
    delete h;
    return NULL;
  }
  FlushToZeroScope fp;
  h->processor.Init(h->large, large, h->small, small);
  h->processor.set_quality(quality);
  h->processor.set_playback_mode(static_cast<clouds::PlaybackMode>(mode));
  h->processor.Prepare();
  return h;
}

void oracle_granular_free(void* processor) {
  GranularHolder* h = static_cast<GranularHolder*>(processor);
  delete[] h->large;
  delete[] h->small;
  delete h;
}

// As the panel's quality button sets it (clouds/ui.cc). The next block is silent while the
// buffers are re-initialized, as on the module. Returns -1 for a quality outside 0 to 3 (nothing
// is changed), 0 otherwise.
int32_t oracle_granular_set_quality(void* processor, int32_t quality) {
  if (quality < 0 || quality > 3) {
    return -1;
  }
  GranularHolder* h = static_cast<GranularHolder*>(processor);
  h->processor.set_quality(quality);
  // The harness calls Prepare after every block; a quality change takes effect at the next one.
  return 0;
}

// As the panel's mode button sets it (clouds/ui.cc). Between the granular and the stretch modes the
// change is benign: Prepare resets the filters and keeps the recording. Returns -1 for a mode
// outside 0 to 1 (nothing is changed), 0 otherwise.
int32_t oracle_granular_set_mode(void* processor, int32_t mode) {
  if (!mode_supported(mode)) {
    return -1;
  }
  static_cast<GranularHolder*>(processor)->processor.set_playback_mode(
      static_cast<clouds::PlaybackMode>(mode));
  return 0;
}

void oracle_granular_set_bypass(void* processor, uint8_t bypass) {
  static_cast<GranularHolder*>(processor)->processor.set_bypass(bypass != 0);
}

void oracle_granular_set_silence(void* processor, uint8_t silence) {
  static_cast<GranularHolder*>(processor)->processor.set_silence(silence != 0);
}

// Process over `size` interleaved stereo int16 frames, then Prepare, as the harness's loop.
// Returns 0 on success, -1 when size is 0, exceeds kMaxBlockSize (the processor's scratch buffers
// hold kMaxBlockSize frames), or is odd at a low-fidelity quality (the 2:1 sample-rate converter
// consumes frames in pairs and would run past the block). Nothing is rendered then.
int32_t oracle_granular_process(
    void* processor,
    const OracleGranularParameters* parameters,
    const int16_t* in,
    int16_t* out,
    size_t size) {
  GranularHolder* h = static_cast<GranularHolder*>(processor);
  if (size == 0 || size > clouds::kMaxBlockSize) {
    return -1;
  }
  if ((h->processor.quality() & 2) && (size & 1)) {
    return -1;
  }
  clouds::Parameters* p = h->processor.mutable_parameters();
  p->position = parameters->position;
  p->size = parameters->size;
  p->pitch = parameters->pitch;
  p->density = parameters->density;
  p->texture = parameters->texture;
  p->dry_wet = parameters->dry_wet;
  p->stereo_spread = parameters->stereo_spread;
  p->feedback = parameters->feedback;
  p->reverb = parameters->reverb;
  p->freeze = parameters->freeze != 0;
  p->trigger = parameters->trigger != 0;
  p->gate = parameters->gate != 0;

  clouds::ShortFrame input[clouds::kMaxBlockSize];
  clouds::ShortFrame output[clouds::kMaxBlockSize];
  for (size_t i = 0; i < size; ++i) {
    input[i].l = in[2 * i];
    input[i].r = in[2 * i + 1];
  }
  {
    FlushToZeroScope fp;
    h->processor.Process(input, output, size);
    h->processor.Prepare();
  }
  for (size_t i = 0; i < size; ++i) {
    out[2 * i] = output[i].l;
    out[2 * i + 1] = output[i].r;
  }
  return 0;
}

}  // extern "C"
