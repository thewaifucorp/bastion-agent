// GeekMagic SmallTV Pro: ESP32-WROOM-32, ST7789 240x240, capacitive touch.
//
// !!! UNVERIFIED !!!
// The display pin mapping of the Pro is NOT publicly documented. The values
// below are the Ultra's, used as a starting point. They MUST be confirmed on
// real hardware (continuity-test the display flex against the ESP32 pads, or
// dump the stock firmware) before relying on this build. If the screen stays
// blank, this block is the first place to look. The touch pad is assumed to be
// on GPIO32 (T9).
//
// Plain #defines only: this header is force-included (-include).
#pragma once

#define BASTION_BOARD "pro"
#define BASTION_TOUCH 1
#define BASTION_MAX_BYTES 8192
#define BASTION_MAX_SESSIONS 8
#define BASTION_MIN_HEAP 40000

// ---- display pins: CONFIRM ON HARDWARE ------------------------------------
#define TFT_MOSI 13
#define TFT_SCLK 14
#define TFT_DC 0
#define TFT_RST 2

#define PIN_TFT_BL 5
#define BL_ACTIVE_LOW 1
// ---------------------------------------------------------------------------

// Touch: ESP32 capacitive sensing. The reading DROPS when touched; a touch is
// a reading more than BASTION_TOUCH_DELTA below the baseline measured at boot.
#define BASTION_TOUCH_PIN 32
#ifndef BASTION_TOUCH_DELTA
#define BASTION_TOUCH_DELTA 40
#endif
