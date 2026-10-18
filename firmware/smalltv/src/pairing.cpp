#include "pairing.h"
#include "net.h"
#include <ArduinoJson.h>

namespace pairing {

static const uint8_t MAX_WRONG = 5;
static const uint32_t LOCKOUT_MS = 60000UL;

static store::Paired cur;
static bool isPaired = false;
static uint16_t pairCode = 0;
static uint32_t showUntil = 0;
static uint8_t wrong = 0;
static uint32_t lockedUntil = 0;

static void rotate() { pairCode = (uint16_t)(hwRandom() % 10000UL); }

static String randomHex32() {
  char b[33];
  for (int i = 0; i < 4; i++) snprintf(b + i * 8, 9, "%08x", (unsigned)hwRandom());
  return String(b);
}

void begin() {
  isPaired = store::loadPaired(cur);
  rotate();
}

bool paired() { return isPaired; }
const store::Paired& cfg() { return cur; }
uint16_t code() { return pairCode; }
bool codeVisible() { return !isPaired || deadlineOpen(showUntil); }
void showCode(uint32_t ms) { showUntil = deadlineIn(ms); }
void hideCode() { showUntil = 0; }

bool tokenOk(const String& header) {
  if (!isPaired || !header.startsWith("Bearer ")) return false;
  String t = header.substring(7);
  t.trim();
  if (t.length() != cur.token.length()) return false;
  uint8_t diff = 0;
  for (size_t i = 0; i < t.length(); i++) diff |= (uint8_t)(t[i] ^ cur.token[i]);
  return diff == 0;
}

const char* capsJson() {
  static char buf[112];
  snprintf(buf, sizeof(buf),
           "{\"w\":240,\"h\":240,\"touch\":%s,\"max_bytes\":%d,\"max_sessions\":%d}",
           BASTION_TOUCH ? "true" : "false", BASTION_MAX_BYTES, BASTION_MAX_SESSIONS);
  return buf;
}

static void reply(WebSrv& s, int code, const char* body) { s.send(code, "application/json", body); }

void handlePair(WebSrv& s) {
  if (lockedUntil) {
    if (deadlineOpen(lockedUntil)) {
      s.sendHeader("Retry-After", String((lockedUntil - millis()) / 1000 + 1));
      reply(s, 429, "{\"error\":\"locked\"}");
      return;
    }
    lockedUntil = 0;
  }
  if (!codeVisible()) {
    reply(s, 403, "{\"error\":\"pairing_closed\"}");
    return;
  }

  JsonDocument d;
  if (deserializeJson(d, s.arg("plain"))) {
    reply(s, 400, "{\"error\":\"bad_json\"}");
    return;
  }

  char want[8];
  snprintf(want, sizeof(want), "%04u", (unsigned)pairCode);
  String got = d["code"].as<String>();
  if (got != want) {
    if (++wrong >= MAX_WRONG) {
      wrong = 0;
      lockedUntil = deadlineIn(LOCKOUT_MS);
    }
    reply(s, 403, "{\"error\":\"bad_code\"}");
    return;
  }

  String url = d["bastion_url"].as<String>();
  String btok = d["bastion_token"].as<String>();
  String host = d["host"].as<String>();
  if (!url.startsWith("http://") || url.length() > 95 || !btok.length() || btok.length() > 79 ||
      host.length() > 47) {
    reply(s, 400, "{\"error\":\"bad_fields\"}");  // https is not supported on-device
    return;
  }

  store::Paired p;
  p.token = randomHex32();
  p.bastionUrl = url;
  p.bastionToken = btok;
  p.host = host;
  store::savePaired(p);
  cur = p;
  isPaired = true;
  wrong = 0;
  hideCode();
  rotate();

  String out = "{\"id\":\"" + net::deviceId() + "\",\"token\":\"" + p.token + "\",\"caps\":" +
               capsJson() + "}";
  reply(s, 200, out.c_str());
}

}  // namespace pairing
