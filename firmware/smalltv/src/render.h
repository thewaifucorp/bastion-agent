// 240x240 ST7789 rendering with per-region redraw.
#pragma once
#include "platform.h"
#include "state.h"

namespace render {

enum Screen : uint8_t {
  SCR_SETUP = 0,  // Wi-Fi portal + QR
  SCR_PAIR,       // pairing code
  SCR_UPDATE,     // OTA code
  SCR_DISCONNECTED,
  SCR_APPROVAL,
  SCR_MAIN,
  SCR_NONE = 255
};

struct Ctx {
  uint8_t screen;
  bool wifiUp;
  bool paired;
  char ip[20];
  char ap[24];
  char id[20];
  uint16_t pairCode;
  uint16_t updateCode;
  uint8_t view;  // 0 = sessions, 1 = pet
  uint32_t epoch;
  int32_t approvalRemain;  // seconds, <0 = unknown
};

void begin();
void draw(const Ctx& c, const state::Snap& s);

// One-off full-screen message (factory reset, OTA). The next draw() repaints.
void message(const char* line1, const char* line2 = nullptr);

}  // namespace render
