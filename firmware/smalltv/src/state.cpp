#include "state.h"
#include <ArduinoJson.h>

namespace state {

static Snap g;
static bool received = false;
static uint32_t rxMs = 0;

// Only the fields the screen needs are kept by the JSON parser.
static const JsonDocument& filter() {
  static JsonDocument f;
  static bool init = false;
  if (!init) {
    f["now"] = true;
    f["persona"] = true;
    f["pet"]["level"] = true;
    f["pet"]["needs"] = true;
    f["pet"]["cue"] = true;
    f["sessions"][0]["name"] = true;
    f["sessions"][0]["st"] = true;
    f["sessions"][0]["tool"] = true;
    f["sessions"][0]["det"] = true;
    f["more"] = true;
    f["alerts"][0]["id"] = true;
    f["alerts"][0]["kind"] = true;
    f["alerts"][0]["tool"] = true;
    f["alerts"][0]["det"] = true;
    f["alerts"][0]["exp"] = true;
    f["note"]["text"] = true;
    f["note"]["exp"] = true;
    init = true;
  }
  return f;
}

// Copies as printable ASCII (the built-in fonts have no other glyphs).
static void cp(char* dst, size_t n, const char* src) {
  size_t i = 0;
  if (src) {
    for (; i < n - 1 && src[i]; i++) {
      uint8_t c = (uint8_t)src[i];
      dst[i] = (c < 0x20 || c >= 0x7f) ? '?' : (char)c;
    }
  }
  dst[i] = 0;
}

static uint8_t stOf(const char* s) {
  if (!s) return ST_IDLE;
  if (!strcmp(s, "working")) return ST_WORKING;
  if (!strcmp(s, "waiting")) return ST_WAITING;
  if (!strcmp(s, "done")) return ST_DONE;
  if (!strcmp(s, "error")) return ST_ERROR;
  return ST_IDLE;
}

static uint8_t pct(JsonVariantConst v) {
  int x = v.as<int>();
  return x < 0 ? 0 : (x > 100 ? 100 : (uint8_t)x);
}

bool parse(const char* body, size_t len, Snap& out) {
  JsonDocument doc;
  DeserializationError e = deserializeJson(doc, body, len, DeserializationOption::Filter(filter()));
  if (e || !doc.is<JsonObject>()) return false;

  memset(&out, 0, sizeof(out));
  out.seq = doc["seq"].as<uint32_t>();
  out.now = doc["now"].as<uint32_t>();
  cp(out.persona, sizeof(out.persona), doc["persona"].as<const char*>());
  out.level = (uint8_t)doc["pet"]["level"].as<int>();
  out.needs[0] = pct(doc["pet"]["needs"]["water"]);
  out.needs[1] = pct(doc["pet"]["needs"]["food"]);
  out.needs[2] = pct(doc["pet"]["needs"]["play"]);
  out.needs[3] = pct(doc["pet"]["needs"]["rest"]);
  cp(out.cue, sizeof(out.cue), doc["pet"]["cue"].as<const char*>());

  uint16_t extra = 0;
  for (JsonObject s : doc["sessions"].as<JsonArray>()) {
    if (out.n >= BASTION_MAX_SESSIONS) {
      extra++;
      continue;
    }
    Session& d = out.sessions[out.n++];
    cp(d.name, sizeof(d.name), s["name"].as<const char*>());
    cp(d.tool, sizeof(d.tool), s["tool"].as<const char*>());
    cp(d.det, sizeof(d.det), s["det"].as<const char*>());
    d.st = stOf(s["st"].as<const char*>());
  }
  out.more = doc["more"].as<uint16_t>() + extra;

  for (JsonObject a : doc["alerts"].as<JsonArray>()) {
    const char* kind = a["kind"].as<const char*>();
    if (kind && !strcmp(kind, "approval")) {
      out.hasApproval = true;
      cp(out.apId, sizeof(out.apId), a["id"].as<const char*>());
      cp(out.apTool, sizeof(out.apTool), a["tool"].as<const char*>());
      cp(out.apDet, sizeof(out.apDet), a["det"].as<const char*>());
      out.apExp = a["exp"].as<uint32_t>();
      break;
    }
  }

  const char* nt = doc["note"]["text"].as<const char*>();
  if (nt && nt[0]) {
    out.hasNote = true;
    cp(out.note, sizeof(out.note), nt);
    out.noteExp = doc["note"]["exp"].as<uint32_t>();
  }
  return true;
}

void set(const Snap& s) {
  g = s;
  received = true;
  rxMs = millis();
}

const Snap& get() { return g; }
bool everReceived() { return received; }
uint32_t ageMs() { return millis() - rxMs; }
bool fresh() { return received && ageMs() < BASTION_STALE_MS; }

uint32_t epochNow() {
  if (!received || g.now == 0) return 0;
  return g.now + ageMs() / 1000;
}

}  // namespace state
