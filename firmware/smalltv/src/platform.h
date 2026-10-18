// Cross-platform includes and small shims for ESP8266 / ESP32.
#pragma once
#include <Arduino.h>

#ifndef BASTION_BOARD
#error "No board header: build with `pio run -e ultra` or `-e pro`."
#endif

#define BASTION_FW "0.1.0"

#if defined(ESP8266)
#include <ESP8266WiFi.h>
#include <ESP8266WebServer.h>
#include <ESP8266mDNS.h>
#include <ESP8266HTTPClient.h>
#include <LittleFS.h>
#include <Updater.h>
typedef ESP8266WebServer WebSrv;
inline uint32_t hwRandom() { return RANDOM_REG32; }
#else
#include <WiFi.h>
#include <WebServer.h>
#include <ESPmDNS.h>
#include <HTTPClient.h>
#include <Preferences.h>
#include <Update.h>
#include <esp_system.h>
typedef WebServer WebSrv;
inline uint32_t hwRandom() { return esp_random(); }
#endif

#include <WiFiClient.h>

// Wall time shown in the top bar is UTC plus this offset (minutes).
#ifndef BASTION_TZ_MIN
#define BASTION_TZ_MIN 0
#endif

// Seconds without a push before the "disconnected" screen.
#define BASTION_STALE_MS 30000UL

// True while `until` (a millis() deadline, 0 = closed) has not passed.
inline bool deadlineOpen(uint32_t until) { return until != 0 && (int32_t)(millis() - until) < 0; }
inline uint32_t deadlineIn(uint32_t ms) { uint32_t t = millis() + ms; return t ? t : 1; }
