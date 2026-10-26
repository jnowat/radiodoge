// Raw LoRa profile used by CMD_RAW_TX / CMD_RAW_RX_MODE (FW 12).
// Lives in a header so Arduino auto-generated prototypes can see it.
#pragma once
#include <stdint.h>

struct RawLoRaProfile {
  uint8_t  sync;
  uint8_t  sf, bw, cr;
  uint32_t freq_hz;
  int8_t   power;
  uint16_t preamble;
};
