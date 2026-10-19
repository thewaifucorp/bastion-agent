// Persistent storage: LittleFS (ESP8266) / Preferences (ESP32).
#pragma once
#include "platform.h"

namespace store {

struct Paired {
  String token;         // device's own push token (Bastion -> device)
  String bastionUrl;    // where to POST actions
  String bastionToken;  // sent as x-bastion-device-token
  String host;
  bool valid() const { return token.length() == 32 && bastionUrl.length() > 0; }
};

void begin();

bool loadPaired(Paired& out);
void savePaired(const Paired& p);
void clearPaired();

bool loadWifi(String& ssid, String& pass);
void saveWifi(const String& ssid, const String& pass);

// Quick-power-cycle counter for the factory reset gesture.
uint8_t bumpBootCount();
void clearBootCount();

// Wipes pairing, Wi-Fi credentials and the boot counter.
void factoryReset();

}  // namespace store
