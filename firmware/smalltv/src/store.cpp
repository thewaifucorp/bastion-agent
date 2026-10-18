#include "store.h"
#include <ArduinoJson.h>

namespace store {

#if defined(ESP8266)

static const char* PAIRED_FILE = "/paired.json";
static const char* BOOT_FILE = "/boot";

void begin() {
  if (!LittleFS.begin()) {
    LittleFS.format();
    LittleFS.begin();
  }
}

bool loadPaired(Paired& out) {
  File f = LittleFS.open(PAIRED_FILE, "r");
  if (!f) return false;
  JsonDocument d;
  DeserializationError e = deserializeJson(d, f);
  f.close();
  if (e) return false;
  out.token = d["token"].as<String>();
  out.bastionUrl = d["url"].as<String>();
  out.bastionToken = d["btoken"].as<String>();
  out.host = d["host"].as<String>();
  return out.valid();
}

void savePaired(const Paired& p) {
  JsonDocument d;
  d["token"] = p.token;
  d["url"] = p.bastionUrl;
  d["btoken"] = p.bastionToken;
  d["host"] = p.host;
  File f = LittleFS.open(PAIRED_FILE, "w");
  if (!f) return;
  serializeJson(d, f);
  f.close();
}

void clearPaired() { LittleFS.remove(PAIRED_FILE); }

// Credentials live in the SDK's own persistent Wi-Fi config.
bool loadWifi(String& ssid, String& pass) {
  ssid = WiFi.SSID();
  pass = WiFi.psk();
  return ssid.length() > 0;
}

void saveWifi(const String& ssid, const String& pass) {
  WiFi.persistent(true);
  WiFi.begin(ssid.c_str(), pass.c_str());
}

uint8_t bumpBootCount() {
  uint8_t n = 0;
  File f = LittleFS.open(BOOT_FILE, "r");
  if (f) {
    n = (uint8_t)f.read();
    f.close();
  }
  if (n < 250) n++;
  File w = LittleFS.open(BOOT_FILE, "w");
  if (w) {
    w.write(n);
    w.close();
  }
  return n;
}

void clearBootCount() { LittleFS.remove(BOOT_FILE); }

void factoryReset() {
  LittleFS.format();
  WiFi.persistent(true);
  WiFi.disconnect(true);
  ESP.eraseConfig();
}

#else  // ESP32

static Preferences prefs;

void begin() { prefs.begin("bastion", false); }

bool loadPaired(Paired& out) {
  out.token = prefs.getString("tok", "");
  out.bastionUrl = prefs.getString("url", "");
  out.bastionToken = prefs.getString("btok", "");
  out.host = prefs.getString("host", "");
  return out.valid();
}

void savePaired(const Paired& p) {
  prefs.putString("tok", p.token);
  prefs.putString("url", p.bastionUrl);
  prefs.putString("btok", p.bastionToken);
  prefs.putString("host", p.host);
}

void clearPaired() {
  prefs.remove("tok");
  prefs.remove("url");
  prefs.remove("btok");
  prefs.remove("host");
}

bool loadWifi(String& ssid, String& pass) {
  ssid = prefs.getString("ssid", "");
  pass = prefs.getString("pass", "");
  return ssid.length() > 0;
}

void saveWifi(const String& ssid, const String& pass) {
  prefs.putString("ssid", ssid);
  prefs.putString("pass", pass);
}

uint8_t bumpBootCount() {
  uint8_t n = prefs.getUChar("boots", 0);
  if (n < 250) n++;
  prefs.putUChar("boots", n);
  return n;
}

void clearBootCount() { prefs.remove("boots"); }

void factoryReset() { prefs.clear(); }

#endif

}  // namespace store
