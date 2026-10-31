// Written by transit as a stand-in for the texture processor's codec driver
// header (beads/drivers/codec.h, not vendored: it is STM32 hardware code); it
// holds no upstream code. The input
// stage (beads/gain_control.h) includes it for one type, the codec's raw
// frame: two signed 32-bit samples, left and right, as the module's codec
// delivers them. Only build.rs's include path makes the oracle find this file.
#ifndef BEADS_DRIVERS_CODEC_H_
#define BEADS_DRIVERS_CODEC_H_

#include "stmlib/stmlib.h"

namespace beads {

class Codec {
 public:
  struct Frame {
    int32_t l;
    int32_t r;
  };
};

}  // namespace beads

#endif  // BEADS_DRIVERS_CODEC_H_
