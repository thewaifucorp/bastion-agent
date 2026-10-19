// Bastion presence firmware for GeekMagic SmallTV (ESP8266 "ultra", ESP32 "pro").
#include "platform.h"
#include "actions.h"
#include "api.h"
#include "net.h"
#include "ota.h"
#include "pairing.h"
#include "render.h"
#include "state.h"
#include "store.h"
#include "touch.h"

static const uint8_t RESET_CYCLES = 6;
static const uint32_t BOOT_OK_MS = 10000UL;  // alive this long = not a quick power cycle
static const uint32_t PAIR_SHOW_MS = 120000UL;

static WebSrv server(80);
static render::Ctx ctx;
static char dismissed[16] = "";  // approval already answered from this device
static bool bootCleared = false;
static uint32_t ipAt = 0;

static bool approvalActive(const state::Snap& s, uint32_t epoch) {
  if (!s.hasApproval) return false;
  if (!strcmp(s.apId, dismissed)) return false;
  return s.apExp == 0 || epoch == 0 || epoch < s.apExp;
}

static uint8_t chooseScreen(const state::Snap& s, uint32_t epoch) {
  if (ota::codeVisible()) return render::SCR_UPDATE;
  if (net::portalActive() && !net::connected()) return render::SCR_SETUP;
  if (pairing::codeVisible() && net::connected()) return render::SCR_PAIR;
  if (!state::fresh()) return render::SCR_DISCONNECTED;
  if (approvalActive(s, epoch)) return render::SCR_APPROVAL;
  return render::SCR_MAIN;
}

static void handleTouch(touch::Event ev, const state::Snap& s) {
  switch (ctx.screen) {
    case render::SCR_APPROVAL: {
      bool ok = actions::send(ev == touch::Tap ? "approve" : "deny", s.apId);
      if (ok) strlcpy(dismissed, s.apId, sizeof(dismissed));
      break;
    }
    case render::SCR_PAIR:
      if (ev == touch::Tap && pairing::paired()) pairing::hideCode();
      break;
    case render::SCR_MAIN:
      if (ev == touch::Tap) ctx.view ^= 1;
      else pairing::showCode(PAIR_SHOW_MS);
      break;
    default:
      break;
  }
}

void setup() {
  store::begin();
  render::begin();

  if (store::bumpBootCount() >= RESET_CYCLES) {
    render::message("Factory reset", "Restarting...");
    store::factoryReset();
    delay(1500);
    ESP.restart();
  }

  net::begin(server);
  pairing::begin();
  api::begin(server);
  touch::begin();
  server.begin();

  memset(&ctx, 0, sizeof(ctx));
  strlcpy(ctx.id, net::deviceId().c_str(), sizeof(ctx.id));
  strlcpy(ctx.ap, net::apName().c_str(), sizeof(ctx.ap));
}

void loop() {
  server.handleClient();
  net::loop();
  ota::loop();

  if (!bootCleared && millis() > BOOT_OK_MS) {
    store::clearBootCount();
    bootCleared = true;
  }

  if (millis() - ipAt > 2000 || ipAt == 0) {
    ipAt = millis() | 1;
    strlcpy(ctx.ip, net::connected() ? WiFi.localIP().toString().c_str() : "", sizeof(ctx.ip));
  }

  const state::Snap& s = state::get();
  uint32_t epoch = state::epochNow();
  if (!s.hasApproval) dismissed[0] = 0;

  ctx.screen = chooseScreen(s, epoch);
  ctx.wifiUp = net::connected();
  ctx.paired = pairing::paired();
  ctx.pairCode = pairing::code();
  ctx.updateCode = ota::code();
  ctx.epoch = epoch;
  ctx.approvalRemain = (s.apExp && epoch) ? (int32_t)(s.apExp - epoch) : -1;

  touch::Event ev = touch::poll();
  if (ev != touch::None) handleTouch(ev, s);

  render::draw(ctx, s);
  delay(2);
}
