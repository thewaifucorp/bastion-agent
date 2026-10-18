#include "api.h"
#include "net.h"
#include "ota.h"
#include "pairing.h"
#include "state.h"

namespace api {

static WebSrv* S = nullptr;

static void json(int code, const char* body) { S->send(code, "application/json", body); }

static bool authed() { return pairing::tokenOk(S->header("Authorization")); }

static void onInfo() {
  String out = "{\"id\":\"" + net::deviceId() + "\",\"fw\":\"" BASTION_FW "\",\"board\":\"" BASTION_BOARD
               "\",\"paired\":" + (pairing::paired() ? "true" : "false") + ",\"caps\":" +
               pairing::capsJson() + "}";
  json(200, out.c_str());
}

static void onPair() { pairing::handlePair(*S); }

// Owner asks a paired device to show the pairing code again (re-pair replaces).
static void onPairOpen() {
  if (!authed()) return json(401, "{\"error\":\"unauthorized\"}");
  pairing::showCode(120000UL);
  json(200, "{\"ok\":true,\"expires_in\":120}");
}

static void onState() {
  if (!authed()) return json(401, "{\"error\":\"unauthorized\"}");

  const String& body = S->arg("plain");
  size_t n = body.length();
  size_t declared = (size_t)S->header("Content-Length").toInt();
  if (declared > n) n = declared;
  if (n > BASTION_MAX_BYTES) return json(413, "{\"error\":\"too_large\"}");
  if (ESP.getFreeHeap() < BASTION_MIN_HEAP) return json(503, "{\"error\":\"busy\"}");

  static state::Snap tmp;
  if (!state::parse(body.c_str(), body.length(), tmp)) return json(400, "{\"error\":\"bad_json\"}");
  state::set(tmp);
  S->send(204, "text/plain", "");
}

static void onUpdateOpen() {
  if (!authed()) return json(401, "{\"error\":\"unauthorized\"}");
  ota::handleOpen(*S);
}

void begin(WebSrv& srv) {
  S = &srv;
  static const char* headers[] = {"Authorization", "Content-Length"};
  S->collectHeaders(headers, 2);
  S->on("/api/info", HTTP_GET, onInfo);
  S->on("/api/pair", HTTP_POST, onPair);
  S->on("/api/pair/open", HTTP_POST, onPairOpen);
  S->on("/api/state", HTTP_POST, onState);
  S->on("/update/open", HTTP_POST, onUpdateOpen);
  ota::begin(srv);
}

}  // namespace api
