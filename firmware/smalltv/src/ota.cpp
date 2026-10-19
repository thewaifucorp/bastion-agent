#include "ota.h"
#include "render.h"

namespace ota {

static const uint32_t OPEN_MS = 120000UL;
static const uint8_t MAX_ATTEMPTS = 5;

static WebSrv* S = nullptr;
static uint16_t otaCode = 0;
static uint32_t openUntil = 0;
static uint8_t attempts = 0;
static bool authorized = false;
static bool failed = false;
static uint32_t rebootAt = 0;

bool codeVisible() { return deadlineOpen(openUntil); }
uint16_t code() { return otaCode; }

void handleOpen(WebSrv& srv) {
  otaCode = (uint16_t)(hwRandom() % 10000UL);
  attempts = 0;
  openUntil = deadlineIn(OPEN_MS);
  srv.send(200, "application/json", "{\"ok\":true,\"expires_in\":120}");
}

static bool codeMatches() {
  if (!codeVisible()) return false;
  attempts++;
  char want[8];
  snprintf(want, sizeof(want), "%04u", (unsigned)otaCode);
  bool ok = S->arg("code") == want;
  if (!ok && attempts >= MAX_ATTEMPTS) openUntil = 0;  // too many wrong codes: close
  return ok;
}

static void onUpload() {
  HTTPUpload& up = S->upload();
  switch (up.status) {
    case UPLOAD_FILE_START: {
      failed = false;
      authorized = codeMatches();
      if (!authorized) return;
      render::message("Updating firmware", "Do not unplug");
#if defined(ESP8266)
      WiFiUDP::stopAll();
      uint32_t room = (ESP.getFreeSketchSpace() - 0x1000) & 0xFFFFF000;
      if (!Update.begin(room)) failed = true;
#else
      if (!Update.begin(UPDATE_SIZE_UNKNOWN)) failed = true;
#endif
      if (!failed && S->hasArg("md5")) {
        String md5 = S->arg("md5");
        if (md5.length() != 32 || !Update.setMD5(md5.c_str())) failed = true;
      }
      break;
    }
    case UPLOAD_FILE_WRITE:
      if (authorized && !failed && Update.write(up.buf, up.currentSize) != up.currentSize) failed = true;
      break;
    case UPLOAD_FILE_END:
      if (authorized && !failed && !Update.end(true)) failed = true;  // verifies MD5 when set
      break;
    default:  // aborted
      if (authorized) Update.end();
      failed = true;
      break;
  }
}

static void onDone() {
  if (!authorized) {
    S->send(403, "application/json", "{\"error\":\"bad_code\"}");
  } else if (failed || Update.hasError()) {
    S->send(500, "application/json", "{\"error\":\"update_failed\"}");
    rebootAt = deadlineIn(1000);  // screen is stuck on "Updating": restart to recover
  } else {
    S->send(200, "application/json", "{\"ok\":true}");
    openUntil = 0;
    rebootAt = deadlineIn(800);
  }
  authorized = false;
}

void begin(WebSrv& srv) {
  S = &srv;
  S->on("/update", HTTP_POST, onDone, onUpload);
}

void loop() {
  if (rebootAt && (int32_t)(millis() - rebootAt) >= 0) ESP.restart();
}

}  // namespace ota
