// Capacitive touch (Pro only). On boards without touch, poll() is always None.
#pragma once
#include "platform.h"

namespace touch {

enum Event : uint8_t { None = 0, Tap, Hold };

void begin();  // calibrates the baseline; do not touch the device while booting
Event poll();  // Tap fires on release; Hold fires once after 2 s of contact

}  // namespace touch
