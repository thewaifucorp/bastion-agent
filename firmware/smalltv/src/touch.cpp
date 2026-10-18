#include "touch.h"

namespace touch {

#if BASTION_TOUCH

static const uint32_t HOLD_MS = 2000;
static const uint32_t MIN_TAP_MS = 60;
static const uint8_t DEBOUNCE = 3;  // consecutive 20 ms samples

static uint32_t base = 0;
static bool down = false;
static bool holdFired = false;
static uint8_t cnt = 0;
static uint32_t pressedAt = 0;
static uint32_t lastPoll = 0;

static uint32_t readPad() { return touchRead(BASTION_TOUCH_PIN); }

void begin() {
  uint32_t sum = 0;
  for (int i = 0; i < 16; i++) {
    sum += readPad();
    delay(5);
  }
  base = sum / 16;
}

Event poll() {
  if (millis() - lastPoll < 20) return None;
  lastPoll = millis();

  uint32_t v = readPad();
  bool touched = v + BASTION_TOUCH_DELTA < base;

  if (!down) {
    if (touched) {
      if (++cnt >= DEBOUNCE) {
        down = true;
        holdFired = false;
        cnt = 0;
        pressedAt = millis();
      }
    } else {
      cnt = 0;
      base = (base * 63 + v) / 64;  // follow slow drift while idle
    }
    return None;
  }

  if (touched) {
    cnt = 0;
    if (!holdFired && millis() - pressedAt >= HOLD_MS) {
      holdFired = true;
      return Hold;
    }
    return None;
  }
  if (++cnt >= DEBOUNCE) {
    down = false;
    cnt = 0;
    if (!holdFired && millis() - pressedAt >= MIN_TAP_MS) return Tap;
  }
  return None;
}

#else

void begin() {}
Event poll() { return None; }

#endif

}  // namespace touch
