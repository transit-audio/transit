// C ABI around the vendored macro-oscillator DSP, for the transit oracle crate (ADR 0033, #194).
// Dev-only: built by build.rs for tests and CI, never linked into the app.
//
// Every entry point is called from Rust only while the crate's process-wide session lock is held
// (src/ffi.rs), because stmlib::Random keeps its state in one static.

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <new>

#if defined(__x86_64__) || defined(_M_X64) || defined(__i386__) || defined(_M_IX86)
#define TRANSIT_ORACLE_X86 1
#include <xmmintrin.h>
#endif

#include "plaits/dsp/voice.h"
#include "plaits/resources.h"
#include "stmlib/utils/random.h"

namespace {

// As the firmware and its desktop test harness: one 16 KB arena per voice or engine.
const size_t kArenaSize = 16384;
const int kEngineCount = 24;

// Mirrors the voice Patch (plaits/dsp/voice.h) with fixed-width fields.
struct OraclePatch {
  float note;
  float harmonics;
  float timbre;
  float morph;
  float frequency_modulation_amount;
  float timbre_modulation_amount;
  float morph_modulation_amount;
  int32_t engine;
  float decay;
  float lpg_colour;
};

// Mirrors the voice Modulations (plaits/dsp/voice.h) with fixed-width fields (bools as uint8_t).
struct OracleModulations {
  float engine;
  float note;
  float frequency;
  float harmonics;
  float timbre;
  float morph;
  float trigger;
  float level;
  uint8_t frequency_patched;
  uint8_t timbre_patched;
  uint8_t morph_patched;
  uint8_t trigger_patched;
  uint8_t level_patched;
};

// Mirrors EngineParameters (plaits/dsp/engine/engine.h).
struct OracleEngineParameters {
  int32_t trigger;
  float note;
  float timbre;
  float morph;
  float harmonics;
  float accent;
};

struct OracleVoice {
  alignas(16) char arena[kArenaSize];
  stmlib::BufferAllocator allocator;
  plaits::Voice voice;
};

// plaits::Engine has no virtual destructor, so each engine is owned through a holder of its
// concrete type and deleted through the holder.
struct EngineHolder {
  virtual ~EngineHolder() { }
  virtual plaits::Engine* get() = 0;
  alignas(16) char arena[kArenaSize];
  stmlib::BufferAllocator allocator;
  int index;
};

template<typename T>
struct TypedHolder : EngineHolder {
  T engine;
  plaits::Engine* get() { return &engine; }
};

// The order of Voice::Init (plaits/dsp/voice.cc).
EngineHolder* NewEngine(int index) {
  using namespace plaits;
  switch (index) {
    case 0: return new (std::nothrow) TypedHolder<VirtualAnalogVCFEngine>();
    case 1: return new (std::nothrow) TypedHolder<PhaseDistortionEngine>();
    case 2:
    case 3:
    case 4: return new (std::nothrow) TypedHolder<SixOpEngine>();
    case 5: return new (std::nothrow) TypedHolder<WaveTerrainEngine>();
    case 6: return new (std::nothrow) TypedHolder<StringMachineEngine>();
    case 7: return new (std::nothrow) TypedHolder<ChiptuneEngine>();
    case 8: return new (std::nothrow) TypedHolder<VirtualAnalogEngine>();
    case 9: return new (std::nothrow) TypedHolder<WaveshapingEngine>();
    case 10: return new (std::nothrow) TypedHolder<FMEngine>();
    case 11: return new (std::nothrow) TypedHolder<GrainEngine>();
    case 12: return new (std::nothrow) TypedHolder<AdditiveEngine>();
    case 13: return new (std::nothrow) TypedHolder<WavetableEngine>();
    case 14: return new (std::nothrow) TypedHolder<ChordEngine>();
    case 15: return new (std::nothrow) TypedHolder<SpeechEngine>();
    case 16: return new (std::nothrow) TypedHolder<SwarmEngine>();
    case 17: return new (std::nothrow) TypedHolder<NoiseEngine>();
    case 18: return new (std::nothrow) TypedHolder<ParticleEngine>();
    case 19: return new (std::nothrow) TypedHolder<StringEngine>();
    case 20: return new (std::nothrow) TypedHolder<ModalEngine>();
    case 21: return new (std::nothrow) TypedHolder<BassDrumEngine>();
    case 22: return new (std::nothrow) TypedHolder<SnareDrumEngine>();
    case 23: return new (std::nothrow) TypedHolder<HiHatEngine>();
    default: return NULL;
  }
}

// The already-enveloped flags Voice::Init registers (plaits/dsp/voice.cc), in the same order.
const bool kAlreadyEnveloped[kEngineCount] = {
  false, false, true, true, true, false, false, false,
  false, false, false, false, false, false, false, false,
  false, false, false, true, true, true, true, true,
};

// The firmware's desktop test harness turns flush-to-zero on (plaits/test/plaits_test.cc). The
// mode is switched only around each call into the vendored code and restored before returning to
// Rust, which assumes the default floating-point environment everywhere it runs. On x86 this is
// MXCSR FTZ (denormal results flush; inputs are not zeroed, as upstream). On aarch64 it is FPCR.FZ,
// which also zeroes denormal inputs. Elsewhere the mode is left alone.
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

}  // namespace

extern "C" {

// The firmware's processing constants, so the Rust side can check its copies.
size_t oracle_block_size() { return plaits::kBlockSize; }
size_t oracle_max_block_size() { return plaits::kMaxBlockSize; }
int32_t oracle_engine_count() { return kEngineCount; }

// Upstream runs at 48 kHz only (plaits/dsp/dsp.h, kSampleRate). Any other rate returns NULL.
void* oracle_voice_new(float sample_rate) {
  if (sample_rate != plaits::kSampleRate) {
    return NULL;
  }
  OracleVoice* v = new (std::nothrow) OracleVoice();
  if (v == NULL) {
    return NULL;
  }
  v->allocator.Init(v->arena, kArenaSize);
  FlushToZeroScope fp;
  v->voice.Init(&v->allocator);
  return v;
}

void oracle_voice_free(void* voice) {
  delete static_cast<OracleVoice*>(voice);
}

// Renders `size` interleaved out/aux int16 frames (2 * size values). Returns 0 on success, -1
// when size is 0 or exceeds kMaxBlockSize (nothing is rendered; the firmware divides by it).
int32_t oracle_voice_render(
    void* voice,
    const OraclePatch* patch,
    const OracleModulations* modulations,
    int16_t* frames,
    size_t size) {
  if (size == 0 || size > plaits::kMaxBlockSize) {
    return -1;
  }
  plaits::Patch p;
  p.note = patch->note;
  p.harmonics = patch->harmonics;
  p.timbre = patch->timbre;
  p.morph = patch->morph;
  p.frequency_modulation_amount = patch->frequency_modulation_amount;
  p.timbre_modulation_amount = patch->timbre_modulation_amount;
  p.morph_modulation_amount = patch->morph_modulation_amount;
  p.engine = patch->engine;
  p.decay = patch->decay;
  p.lpg_colour = patch->lpg_colour;

  plaits::Modulations m;
  m.engine = modulations->engine;
  m.note = modulations->note;
  m.frequency = modulations->frequency;
  m.harmonics = modulations->harmonics;
  m.timbre = modulations->timbre;
  m.morph = modulations->morph;
  m.trigger = modulations->trigger;
  m.level = modulations->level;
  m.frequency_patched = modulations->frequency_patched != 0;
  m.timbre_patched = modulations->timbre_patched != 0;
  m.morph_patched = modulations->morph_patched != 0;
  m.trigger_patched = modulations->trigger_patched != 0;
  m.level_patched = modulations->level_patched != 0;

  plaits::Voice::Frame out[plaits::kMaxBlockSize];
  {
    FlushToZeroScope fp;
    static_cast<OracleVoice*>(voice)->voice.Render(p, m, out, size);
  }
  for (size_t i = 0; i < size; ++i) {
    frames[2 * i] = out[i].out;
    frames[2 * i + 1] = out[i].aux;
  }
  return 0;
}

// One engine on its own arena, initialized as Voice::Render does when it switches to it:
// Init, LoadUserData (the built-in FM banks for 2 to 4), Reset.
void* oracle_engine_new(int32_t index) {
  EngineHolder* h = NewEngine(index);
  if (h == NULL) {
    return NULL;
  }
  h->index = index;
  h->allocator.Init(h->arena, kArenaSize);
  plaits::Engine* e = h->get();
  FlushToZeroScope fp;
  e->Init(&h->allocator);
  e->LoadUserData(index >= 2 && index <= 4 ? plaits::fm_patches_table[index - 2] : NULL);
  e->Reset();
  return h;
}

void oracle_engine_free(void* engine) {
  delete static_cast<EngineHolder*>(engine);
}

// Float out and aux before any post-processing (no gain, limiter or LPG). Returns 0 on success,
// -1 when size is 0 or exceeds kMaxBlockSize. `already_enveloped` receives the engine's per-call report,
// seeded with the flag Voice::Init registers for it.
int32_t oracle_engine_render(
    void* engine,
    const OracleEngineParameters* parameters,
    float* out,
    float* aux,
    size_t size,
    uint8_t* already_enveloped) {
  if (size == 0 || size > plaits::kMaxBlockSize) {
    return -1;
  }
  EngineHolder* h = static_cast<EngineHolder*>(engine);
  plaits::EngineParameters p;
  p.trigger = parameters->trigger;
  p.note = parameters->note;
  p.timbre = parameters->timbre;
  p.morph = parameters->morph;
  p.harmonics = parameters->harmonics;
  p.accent = parameters->accent;
  bool enveloped = kAlreadyEnveloped[h->index];
  {
    FlushToZeroScope fp;
    h->get()->Render(p, out, aux, size, &enveloped);
  }
  *already_enveloped = enveloped ? 1 : 0;
  return 0;
}

// Writes stmlib's process-wide random state (stmlib/utils/random.h).
void oracle_seed(uint32_t seed) { stmlib::Random::Seed(seed); }
uint32_t oracle_rng_state() { return stmlib::Random::state(); }

}  // extern "C"
