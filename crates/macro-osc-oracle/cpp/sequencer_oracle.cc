// C ABI around the vendored random sequencer (the eurorack `marbles/random`
// and `marbles/ramp` generators: the t generator's gates, the X and Y
// generators' voltages through the quantizer, deja vu, and the scale
// recorder), for the transit oracle crate (ADR 0033, #202). Dev-only: built by
// build.rs for tests and CI, never linked into the app.
//
// Every entry point is called from Rust only while the crate's process-wide
// session lock is held (src/ffi.rs). This wrapper keeps no static state of its
// own: each sequencer owns its random generator, as the module's single global
// does.
//
// The wrapper drives the generators as the firmware's main loop does
// (marbles/marbles.cc, `Process`, vendored for reference and not compiled):
// at 32 kHz, in the module's blocks of 5 frames, the t generator first, then
// the X and Y generators, with the knob values the CV reader would hand them.
// The CV reader's filtering and calibration, the UI, the clock-input
// self-patching detector and the DAC are left out: the caller passes the
// parameters, the clock sources and the gate flags directly. The gate outputs
// are the generators' (the firmware delays them two samples at the DAC to line
// them up with the CV outputs; that delay is not applied here).

#include <cstddef>
#include <cstdint>
#include <new>

#include "ftz_scope.h"
#include "marbles/random/random_generator.h"
#include "marbles/random/random_stream.h"
#include "marbles/random/t_generator.h"
#include "marbles/random/x_y_generator.h"
#include "marbles/scale_recorder.h"
#include "stmlib/dsp/hysteresis_quantizer.h"

using namespace marbles;

namespace {

using transit_oracle::FlushToZeroScope;

// The module's sample rate and block size (marbles/marbles.cc `kSampleRate`,
// marbles/io_buffer.h `kBlockSize`).
const int kSampleRate = 32000;
const size_t kFirmwareBlockSize = 5;
const size_t kNumScales = 6;

// marbles/settings.h `DejaVuState` (that header needs the hardware's drivers).
enum TransitDejaVuState {
  TRANSIT_DEJA_VU_OFF,
  TRANSIT_DEJA_VU_ON,
  TRANSIT_DEJA_VU_LOCKED
};

// From marbles/marbles.cc, as fetched: the Y generator's clock divisions
// (`y_divider_ratios`) and the deja vu loop lengths the length knob steps
// through (`loop_length`). tests/sequencer_self_floor.rs reads both arrays
// from the vendored marbles.cc and checks these copies against them.
const Ratio kYDividerRatios[] = {
  { 1, 64 }, { 1, 48 }, { 1, 32 }, { 1, 24 }, { 1, 16 }, { 1, 12 },
  { 1, 8 }, { 1, 6 }, { 1, 4 }, { 1, 3 }, { 1, 2 }, { 1, 1 },
};

const int kLoopLength[] = {
  1,
  2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
  3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
  4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
  5, 5, 5, 5,
  6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
  7, 7,
  8, 8, 8, 8, 8, 8, 8, 8, 8,
  10, 10, 10,
  12, 12, 12, 12, 12, 12, 12,
  14, 14,
  16
};
const size_t kLoopLengthCount = sizeof(kLoopLength) / sizeof(int);

// From marbles/settings.cc, as fetched: the six preset scales
// (`preset_scales`: major, minor, pentatonic, pelog, raag bhairav that, raag
// shri). The array has internal linkage there and settings.cc needs the
// hardware's flash storage, so it is copied; the same test checks every value
// against the vendored file.
const Scale kPresetScales[kNumScales] = {
  { 1.0f, 12, {
    { 0.0000f, 255 }, { 0.0833f, 16 }, { 0.1667f, 96 }, { 0.2500f, 24 },
    { 0.3333f, 128 }, { 0.4167f, 64 }, { 0.5000f, 8 }, { 0.5833f, 192 },
    { 0.6667f, 16 }, { 0.7500f, 96 }, { 0.8333f, 24 }, { 0.9167f, 128 } } },
  { 1.0f, 12, {
    { 0.0000f, 255 }, { 0.0833f, 16 }, { 0.1667f, 96 }, { 0.2500f, 128 },
    { 0.3333f, 8 }, { 0.4167f, 64 }, { 0.5000f, 4 }, { 0.5833f, 192 },
    { 0.6667f, 96 }, { 0.7500f, 16 }, { 0.8333f, 128 }, { 0.9167f, 16 } } },
  { 1.0f, 12, {
    { 0.0000f, 255 }, { 0.0833f, 4 }, { 0.1667f, 96 }, { 0.2500f, 4 },
    { 0.3333f, 4 }, { 0.4167f, 140 }, { 0.5000f, 4 }, { 0.5833f, 192 },
    { 0.6667f, 4 }, { 0.7500f, 96 }, { 0.8333f, 4 }, { 0.9167f, 4 } } },
  { 1.0f, 7, {
    { 0.0000f, 255 }, { 0.1275f, 128 }, { 0.2625f, 32 }, { 0.4600f, 8 },
    { 0.5883f, 192 }, { 0.7067f, 64 }, { 0.8817f, 16 } } },
  { 1.0f, 12, {
    { 0.0000f, 255 }, { 0.0752f, 128 }, { 0.1699f, 4 }, { 0.2630f, 4 },
    { 0.3219f, 128 }, { 0.4150f, 64 }, { 0.4918f, 4 }, { 0.5850f, 192 },
    { 0.6601f, 64 }, { 0.7549f, 4 }, { 0.8479f, 4 }, { 0.9069f, 64 } } },
  { 1.0f, 12, {
    { 0.0000f, 255 }, { 0.0752f, 4 }, { 0.1699f, 128 }, { 0.2630f, 64 },
    { 0.3219f, 4 }, { 0.4150f, 128 }, { 0.4918f, 4 }, { 0.5850f, 192 },
    { 0.6601f, 4 }, { 0.7549f, 64 }, { 0.8479f, 128 }, { 0.9069f, 4 } } },
};

// What the main loop reads each block: the knobs (after the CV reader), the
// settings' state bytes, the explicit reset gates and the clock routing.
struct OracleSequencerControls {
  int32_t t_model;             // TGeneratorModel, 0 to 6 (the module offers 0 to 5)
  int32_t t_range;             // TGeneratorRange, 0 to 2
  float t_rate;                // semitones around 2 Hz (the knob: -60 to 60)
  float t_bias;                // 0 to 1
  float t_jitter;              // 0 to 1
  float deja_vu;               // the knob, 0 to 1 (the center dead band applies)
  float deja_vu_length;        // the knob, 0 to 1 (stepped through kLoopLength)
  int32_t t_deja_vu;           // DejaVuState, 0 to 2
  int32_t x_deja_vu;           // DejaVuState, 0 to 2
  int32_t t_pulse_width_mean;  // state byte, 0 to 255
  int32_t t_pulse_width_std;   // state byte, 0 to 255
  int32_t x_control_mode;      // ControlMode, 0 to 2
  int32_t x_range;             // VoltageRange, 0 to 2
  int32_t x_register_mode;     // 0 or 1
  float x_register_value;      // the note filter's output, 0 to 1
  float x_spread;              // 0 to 1
  float x_bias;                // 0 to 1
  float x_steps;               // 0 to 1
  int32_t x_scale;             // 0 to 5
  int32_t y_spread;            // state byte, 0 to 255
  int32_t y_bias;              // state byte, 0 to 255
  int32_t y_steps;             // state byte, 0 to 255
  int32_t y_divider;           // state byte, 0 to 255
  int32_t y_range;             // VoltageRange, 0 to 2
  int32_t t_reset;             // the t section's explicit reset fires this block
  int32_t x_reset;             // the X section's explicit reset fires this block
  int32_t t_external;          // the t clock input is patched
  int32_t xy_clock_source;     // ClockSource, 0 to 4 (4: the X clock input)
};

// The module's generators. On the module they are globals in static storage,
// so every member Init leaves unwritten starts at zero: the Y generator's ramp
// divider never initializes `reset_at_next_pulse_`, which its first wrap reads
// (an upstream finding). Allocating with `new SequencerHolder()`
// value-initializes the holder (no user-provided constructor), so it is
// zeroed before the members' constructors run; build.rs passes
// -flifetime-dse=1 to GCC so that zeroing is kept.
struct SequencerHolder {
  RandomGenerator random_generator;
  RandomStream random_stream;
  TGenerator t_generator;
  XYGenerator xy_generator;
  stmlib::HysteresisQuantizer2 deja_vu_length_quantizer;
  GroupSettings x;
  GroupSettings y;
  float ramp_buffer[kFirmwareBlockSize * 4];
  bool gates[kFirmwareBlockSize * 2];
  float voltages[kFirmwareBlockSize * 4];
  uint8_t flush;
};

struct RecorderHolder {
  ScaleRecorder recorder;
};

bool Unit(float x) { return x >= 0.0f && x <= 1.0f; }
bool Within(int32_t x, int32_t lo, int32_t hi) { return x >= lo && x <= hi; }

bool ControlsInRange(const OracleSequencerControls& c) {
  return Within(c.t_model, 0, 6) && Within(c.t_range, 0, 2) &&
      c.t_rate >= -64.0f && c.t_rate <= 64.0f &&
      Unit(c.t_bias) && Unit(c.t_jitter) && Unit(c.deja_vu) &&
      Unit(c.deja_vu_length) && Within(c.t_deja_vu, 0, 2) &&
      Within(c.x_deja_vu, 0, 2) && Within(c.t_pulse_width_mean, 0, 255) &&
      Within(c.t_pulse_width_std, 0, 255) && Within(c.x_control_mode, 0, 2) &&
      Within(c.x_range, 0, 2) && Within(c.x_register_mode, 0, 1) &&
      Unit(c.x_register_value) && Unit(c.x_spread) && Unit(c.x_bias) &&
      Unit(c.x_steps) && Within(c.x_scale, 0, kNumScales - 1) &&
      Within(c.y_spread, 0, 255) && Within(c.y_bias, 0, 255) &&
      Within(c.y_steps, 0, 255) && Within(c.y_divider, 0, 255) &&
      Within(c.y_range, 0, 2) && Within(c.t_reset, 0, 1) &&
      Within(c.x_reset, 0, 1) && Within(c.t_external, 0, 1) &&
      Within(c.xy_clock_source, 0, 4);
}

// One block of marbles.cc's Process, from "Filter CV values" on, with the
// parameters given.
void ProcessBlock(
    SequencerHolder* h,
    const OracleSequencerControls& c,
    const stmlib::GateFlags* t_clock,
    const stmlib::GateFlags* xy_clock,
    size_t size) {
  float deja_vu = c.deja_vu;
  // Dead band near 12 o'clock for the deja vu parameter.
  if (deja_vu < 0.47f) {
    deja_vu *= 1.06382978723f;
  } else if (deja_vu > 0.53f) {
    deja_vu = 0.5f + (deja_vu - 0.53f) * 1.06382978723f;
  } else {
    deja_vu = 0.5f;
  }

  Ramps ramps;
  ramps.master = &h->ramp_buffer[0];
  ramps.external = &h->ramp_buffer[kFirmwareBlockSize];
  ramps.slave[0] = &h->ramp_buffer[kFirmwareBlockSize * 2];
  ramps.slave[1] = &h->ramp_buffer[kFirmwareBlockSize * 3];

  int deja_vu_length = h->deja_vu_length_quantizer.Lookup(
      kLoopLength, c.deja_vu_length);

  bool t_section_reset = c.t_reset != 0;

  TGenerator& t = h->t_generator;
  t.set_model(TGeneratorModel(c.t_model));
  t.set_range(TGeneratorRange(c.t_range));
  t.set_rate(c.t_rate);
  t.set_bias(c.t_bias);
  t.set_jitter(c.t_jitter);
  t.set_deja_vu(
      c.t_deja_vu == TRANSIT_DEJA_VU_LOCKED
          ? 0.5f
          : (c.t_deja_vu == TRANSIT_DEJA_VU_ON ? deja_vu : 0.0f));
  t.set_length(deja_vu_length);
  t.set_pulse_width_mean(float(c.t_pulse_width_mean) / 256.0f);
  t.set_pulse_width_std(float(c.t_pulse_width_std) / 256.0f);
  t.Process(
      c.t_external != 0,
      &t_section_reset,
      t_clock,
      ramps,
      h->gates,
      size);

  GroupSettings& x = h->x;
  GroupSettings& y = h->y;
  x.control_mode = ControlMode(c.x_control_mode);
  x.voltage_range = VoltageRange(c.x_range % 3);
  x.register_mode = c.x_register_mode != 0;
  x.register_value = c.x_register_value;
  x.spread = c.x_spread;
  x.bias = c.x_bias;
  x.steps = c.x_steps;
  x.deja_vu = c.x_deja_vu == TRANSIT_DEJA_VU_LOCKED
      ? 0.5f
      : (c.x_deja_vu == TRANSIT_DEJA_VU_ON ? deja_vu : 0.0f);
  x.length = deja_vu_length;
  x.ratio.p = 1;
  x.ratio.q = 1;

  y.control_mode = CONTROL_MODE_IDENTICAL;
  y.voltage_range = VoltageRange(c.y_range);
  y.register_mode = false;
  y.register_value = 0.0f;
  y.spread = float(c.y_spread) / 256.0f;
  y.bias = float(c.y_bias) / 256.0f;
  y.steps = float(c.y_steps) / 256.0f;
  y.deja_vu = 0.0f;
  y.length = 1;
  y.ratio = kYDividerRatios[static_cast<uint16_t>(c.y_divider) * 12 >> 8];

  y.scale_index = x.scale_index = c.x_scale;

  ClockSource xy_clock_source = ClockSource(c.xy_clock_source);
  bool x_section_reset = c.x_reset != 0;
  if (xy_clock_source != CLOCK_SOURCE_EXTERNAL) {
    x_section_reset |= t_section_reset;
  }

  h->xy_generator.Process(
      xy_clock_source,
      x,
      y,
      &x_section_reset,
      xy_clock,
      ramps,
      h->voltages,
      size);
}

}  // namespace

extern "C" {

// The firmware's constants, so the Rust side can check its copies.
size_t oracle_sequencer_block_size() { return kFirmwareBlockSize; }
int32_t oracle_sequencer_sample_rate() { return kSampleRate; }
size_t oracle_sequencer_loop_length_count() { return kLoopLengthCount; }
int32_t oracle_sequencer_loop_length(size_t i) {
  return i < kLoopLengthCount ? kLoopLength[i] : -1;
}
size_t oracle_sequencer_y_divider_count() {
  return sizeof(kYDividerRatios) / sizeof(Ratio);
}
int32_t oracle_sequencer_y_divider(size_t i, int32_t* p, int32_t* q) {
  if (i >= sizeof(kYDividerRatios) / sizeof(Ratio)) {
    return -1;
  }
  *p = kYDividerRatios[i].p;
  *q = kYDividerRatios[i].q;
  return 0;
}

// Preset scale `index` (0 to 5): base interval, number of degrees, and each
// degree's voltage and weight (16 slots each; unused slots are zero).
int32_t oracle_sequencer_preset_scale(
    size_t index,
    float* base_interval,
    int32_t* num_degrees,
    float* voltages,
    uint8_t* weights) {
  if (index >= kNumScales) {
    return -1;
  }
  const Scale& s = kPresetScales[index];
  *base_interval = s.base_interval;
  *num_degrees = s.num_degrees;
  for (int i = 0; i < kMaxDegrees; ++i) {
    voltages[i] = i < s.num_degrees ? s.degree[i].voltage : 0.0f;
    weights[i] = i < s.num_degrees ? s.degree[i].weight : 0;
  }
  return 0;
}

// The generators as the firmware's Init leaves them, the random generator
// seeded with `seed` (the module seeds it with 1), the preset scales loaded.
// Flushes to zero.
void* oracle_sequencer_new(uint32_t seed) {
  SequencerHolder* h = new (std::nothrow) SequencerHolder();
  if (h == NULL) {
    return NULL;
  }
  h->flush = 1;
  FlushToZeroScope fp;
  h->deja_vu_length_quantizer.Init(kLoopLengthCount, 0.25f, false);
  h->random_generator.Init(seed);
  h->random_stream.Init(&h->random_generator);
  h->t_generator.Init(&h->random_stream, static_cast<float>(kSampleRate));
  h->xy_generator.Init(&h->random_stream, static_cast<float>(kSampleRate));
  for (size_t i = 0; i < kNumScales; ++i) {
    h->xy_generator.LoadScale(i, kPresetScales[i]);
  }
  return h;
}

void oracle_sequencer_free(void* sequencer) {
  delete static_cast<SequencerHolder*>(sequencer);
}

// Whether the calls run inside the flush-to-zero scope (the default, as every
// oracle of the family); 0 runs them in the default floating-point mode, an
// evidence switch for the test that asks whether the flush moves a value.
void oracle_sequencer_set_flush(void* sequencer, uint8_t flush) {
  static_cast<SequencerHolder*>(sequencer)->flush = flush;
}

// Loads `num_degrees` degrees (1 to 16, voltages within one base interval,
// ascending) into scale slot `index` (0 to 5) of the three X channels, as the
// firmware does when the scale recorder hands it a scale. Returns -1 on
// arguments the quantizer cannot take (it would keep the old scale) or that
// would leave a quantizer level without a degree (no weight of 255).
int32_t oracle_sequencer_load_scale(
    void* sequencer,
    size_t index,
    float base_interval,
    int32_t num_degrees,
    const float* voltages,
    const uint8_t* weights) {
  if (index >= kNumScales || num_degrees < 1 || num_degrees > kMaxDegrees ||
      !(base_interval > 0.0f && base_interval <= 4.0f)) {
    return -1;
  }
  Scale s;
  s.base_interval = base_interval;
  s.num_degrees = num_degrees;
  bool top = false;
  for (int i = 0; i < num_degrees; ++i) {
    if (!(voltages[i] >= 0.0f && voltages[i] < base_interval) ||
        (i > 0 && !(voltages[i] > voltages[i - 1]))) {
      return -1;
    }
    s.degree[i].voltage = voltages[i];
    s.degree[i].weight = weights[i];
    top |= weights[i] == 255;
  }
  if (!top) {
    return -1;
  }
  SequencerHolder* h = static_cast<SequencerHolder*>(sequencer);
  FlushToZeroScope fp;
  h->xy_generator.LoadScale(index, s);
  return 0;
}

// `size` frames (1 to 5; the module always runs 5) of the main loop. `t_clock`
// (needed when the t clock is patched) and `xy_clock` (needed when the X clock
// source is the X clock input) hold one gate-flags byte per frame. Writes
// three gates per frame to `gates` (t1, t2, t3: 0 or 1) and four voltages per
// frame to `voltages` (X1, X2, X3, Y, volts). Returns 0, or -1 when the size,
// a missing clock or a control is out of range.
int32_t oracle_sequencer_process(
    void* sequencer,
    const OracleSequencerControls* controls,
    const uint8_t* t_clock,
    const uint8_t* xy_clock,
    size_t size,
    uint8_t* gates,
    float* voltages) {
  if (size == 0 || size > kFirmwareBlockSize || !ControlsInRange(*controls)) {
    return -1;
  }
  if ((controls->t_external && t_clock == NULL) ||
      (controls->xy_clock_source == CLOCK_SOURCE_EXTERNAL && xy_clock == NULL)) {
    return -1;
  }
  for (size_t i = 0; t_clock != NULL && i < size; ++i) {
    if (t_clock[i] > 7) {
      return -1;
    }
  }
  for (size_t i = 0; xy_clock != NULL && i < size; ++i) {
    if (xy_clock[i] > 7) {
      return -1;
    }
  }
  SequencerHolder* h = static_cast<SequencerHolder*>(sequencer);
  // The firmware reads its clock inputs from the block buffer; copies keep
  // the caller's untouched and give the t generator a valid pointer either
  // way (it reads the clock only when patched).
  stmlib::GateFlags t_flags[kFirmwareBlockSize] = { 0 };
  stmlib::GateFlags xy_flags[kFirmwareBlockSize] = { 0 };
  for (size_t i = 0; i < size; ++i) {
    t_flags[i] = t_clock != NULL ? t_clock[i] : 0;
    xy_flags[i] = xy_clock != NULL ? xy_clock[i] : 0;
  }
  if (h->flush) {
    FlushToZeroScope fp;
    ProcessBlock(h, *controls, t_flags, xy_flags, size);
  } else {
    ProcessBlock(h, *controls, t_flags, xy_flags, size);
  }
  const float* master = &h->ramp_buffer[0];
  for (size_t i = 0; i < size; ++i) {
    gates[3 * i] = h->gates[2 * i] ? 1 : 0;
    gates[3 * i + 1] = master[i] < 0.5f ? 1 : 0;
    gates[3 * i + 2] = h->gates[2 * i + 1] ? 1 : 0;
    for (size_t k = 0; k < 4; ++k) {
      voltages[4 * i + k] = h->voltages[4 * i + k];
    }
  }
  return 0;
}

// The scale recorder (marbles/scale_recorder.h), on its own: the firmware
// feeds it the note CV while the recording mode is on, a new note on each
// rising gate, the voltage while the gate is high, the note accepted on the
// falling gate, and extracts the scale when the mode ends.
void* oracle_sequencer_recorder_new() {
  RecorderHolder* h = new (std::nothrow) RecorderHolder();
  if (h == NULL) {
    return NULL;
  }
  h->recorder.Init();
  return h;
}

void oracle_sequencer_recorder_free(void* recorder) {
  delete static_cast<RecorderHolder*>(recorder);
}

// One event: 0 a new note at `voltage`, 1 the voltage while held, 2 the note
// accepted (the voltage is ignored). Returns -1 for another kind or a voltage
// outside the module's -5 to 5 volts.
int32_t oracle_sequencer_recorder_event(void* recorder, int32_t kind, float voltage) {
  if (!(voltage >= -5.0f && voltage <= 5.0f)) {
    return -1;
  }
  ScaleRecorder& r = static_cast<RecorderHolder*>(recorder)->recorder;
  FlushToZeroScope fp;
  switch (kind) {
    case 0: r.NewNote(voltage); return 0;
    case 1: r.UpdateVoltage(voltage); return 0;
    case 2: r.AcceptNote(); return 0;
    default: return -1;
  }
}

// ExtractScale: 1 and the scale (as oracle_sequencer_preset_scale writes it)
// when two or more degrees were recorded, else 0 and nothing written.
int32_t oracle_sequencer_recorder_extract(
    void* recorder,
    float* base_interval,
    int32_t* num_degrees,
    float* voltages,
    uint8_t* weights) {
  ScaleRecorder& r = static_cast<RecorderHolder*>(recorder)->recorder;
  Scale s;
  bool ok;
  {
    FlushToZeroScope fp;
    ok = r.ExtractScale(&s);
  }
  if (!ok) {
    return 0;
  }
  *base_interval = s.base_interval;
  *num_degrees = s.num_degrees;
  for (int i = 0; i < kMaxDegrees; ++i) {
    voltages[i] = i < s.num_degrees ? s.degree[i].voltage : 0.0f;
    weights[i] = i < s.num_degrees ? s.degree[i].weight : 0;
  }
  return 1;
}

}  // extern "C"
