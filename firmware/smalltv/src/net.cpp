#include "net.h"
#include "store.h"
#include "pairing.h"
#include <DNSServer.h>

namespace net {

static const uint32_t OFFLINE_REOPEN_MS = 5UL * 60UL * 1000UL;

static WebSrv* S = nullptr;
static DNSServer dns;
static String id, ap, optionsHtml;
static bool portal = false;
static bool haveCreds = false;
static bool mdnsUp = false;
static uint32_t offlineSince = 0;
static uint32_t restartAt = 0;

static String macSuffix() {
  uint8_t m[6];
#if defined(ESP8266)
  WiFi.macAddress(m);
#else
  esp_read_mac(m, ESP_MAC_WIFI_STA);
#endif
  char b[5];
  snprintf(b, sizeof(b), "%02x%02x", m[4], m[5]);
  return String(b);
}

static String esc(const String& s) {
  String o;
  o.reserve(s.length() + 8);
  for (size_t i = 0; i < s.length(); i++) {
    char c = s[i];
    if (c == '<') o += "&lt;";
    else if (c == '>') o += "&gt;";
    else if (c == '&') o += "&amp;";
    else if (c == '\'') o += "&#39;";
    else if (c == '"') o += "&quot;";
    else o += c;
  }
  return o;
}

static void scan() {
  optionsHtml = "";
  int n = WiFi.scanNetworks();
  for (int i = 0; i < n && i < 24; i++) {
    String s = WiFi.SSID(i);
    if (!s.length()) continue;
    String opt = "<option value='" + esc(s) + "'>";
    if (optionsHtml.indexOf(opt) >= 0) continue;
    optionsHtml += opt + esc(s) + "</option>";
  }
  WiFi.scanDelete();
}

static void startPortal() {
  scan();
  WiFi.mode(WIFI_AP_STA);
  WiFi.softAP(ap.c_str());
  dns.start(53, "*", WiFi.softAPIP());
  portal = true;
}

static void stopPortal() {
  dns.stop();
  WiFi.softAPdisconnect(true);
  WiFi.mode(WIFI_STA);
  portal = false;
}

static void startMdns() {
  if (MDNS.begin(id.c_str())) {
    MDNS.addService("bastion-device", "tcp", 80);  // _bastion-device._tcp
    mdnsUp = true;
  }
}

static void handleRoot() {
  if (!portal) {
    S->send(200, "text/plain", id + " (Bastion presence device)");
    return;
  }
  String h =
      "<!doctype html><meta name=viewport content='width=device-width,initial-scale=1'>"
      "<title>Bastion setup</title><style>body{font:16px sans-serif;max-width:360px;margin:2em auto;"
      "padding:0 1em}select,input,button{width:100%;padding:.6em;margin:.4em 0;font-size:1em;"
      "box-sizing:border-box}</style><h2>Bastion device setup</h2>"
      "<form method=post action=/save><label>Wi-Fi network</label><select name=ssid>";
  h += optionsHtml;
  h += "</select><input name=ssid2 placeholder='or type a network name'>"
       "<label>Password</label><input name=pass type=password autocomplete=off>"
       "<button>Save and connect</button></form><p><a href=/rescan>Rescan</a></p>";
  S->send(200, "text/html", h);
}

static void handleRescan() {
  if (portal) scan();
  S->sendHeader("Location", "/");
  S->send(302, "text/plain", "");
}

static void handleSave() {
  if (!portal) {
    S->send(403, "text/plain", "setup is closed");
    return;
  }
  String ssid = S->arg("ssid2");
  if (!ssid.length()) ssid = S->arg("ssid");
  String pass = S->arg("pass");
  if (!ssid.length() || ssid.length() > 32 || pass.length() > 64) {
    S->send(400, "text/plain", "invalid network or password");
    return;
  }
  store::saveWifi(ssid, pass);
  S->send(200, "text/html",
          "<!doctype html><meta name=viewport content='width=device-width,initial-scale=1'>"
          "<p>Saved. The device is restarting and will join <b>" + esc(ssid) +
          "</b>. The pairing code appears on its screen.</p>");
  restartAt = deadlineIn(1500);
}

// Captive-portal probes (generate_204, hotspot-detect, ...) land here.
static void handleNotFound() {
  if (portal) {
    S->sendHeader("Location", "http://" + WiFi.softAPIP().toString() + "/");
    S->send(302, "text/plain", "");
    return;
  }
  S->send(404, "application/json", "{\"error\":\"not_found\"}");
}

void begin(WebSrv& srv) {
  S = &srv;
  String suffix = macSuffix();
  id = "bastion-" + suffix;
  String up = suffix;
  up.toUpperCase();
  ap = "Bastion-Setup-" + up;

#if defined(ESP8266)
  WiFi.hostname(id);
#else
  WiFi.setHostname(id.c_str());
#endif
  WiFi.mode(WIFI_STA);
  WiFi.setAutoReconnect(true);

  S->on("/", HTTP_GET, handleRoot);
  S->on("/rescan", HTTP_GET, handleRescan);
  S->on("/save", HTTP_POST, handleSave);
  S->onNotFound(handleNotFound);

  String ssid, pass;
  haveCreds = store::loadWifi(ssid, pass);
  if (haveCreds) {
#if defined(ESP8266)
    WiFi.persistent(false);
#endif
    WiFi.begin(ssid.c_str(), pass.c_str());
    offlineSince = deadlineIn(0);
  } else {
    startPortal();
  }
}

void loop() {
  if (restartAt && (int32_t)(millis() - restartAt) >= 0) ESP.restart();
  if (portal) dns.processNextRequest();
#if defined(ESP8266)
  if (mdnsUp) MDNS.update();
#endif

  if (connected()) {
    offlineSince = 0;
    if (!mdnsUp) startMdns();
    if (portal && haveCreds) stopPortal();  // reopened portal: network is back
    return;
  }

  mdnsUp = false;
  if (!offlineSince) offlineSince = deadlineIn(0);
  // Keep retrying the saved network, but reopen the portal after 5 min offline
  // — only while unpaired. A paired device never reopens an open setup portal
  // by itself (anyone nearby could repoint it and capture its Bastion token);
  // re-setup takes the physical factory reset (6 quick power cycles).
  if (haveCreds && !portal && !pairing::paired() && millis() - offlineSince > OFFLINE_REOPEN_MS)
    startPortal();
}

bool connected() { return WiFi.status() == WL_CONNECTED; }
bool portalActive() { return portal; }
const String& deviceId() { return id; }
const String& apName() { return ap; }

}  // namespace net
