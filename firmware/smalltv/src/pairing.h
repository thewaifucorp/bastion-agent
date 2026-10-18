// On-screen pairing code, /api/pair and bearer-token checks.
#pragma once
#include "platform.h"
#include "store.h"

namespace pairing {

void begin();

bool paired();
const store::Paired& cfg();

// The 4-digit code is shown while unpaired, or for a while after the owner asks.
bool codeVisible();
uint16_t code();
void showCode(uint32_t ms);
void hideCode();

// Constant-time check of an "Authorization: Bearer <token>" header value.
bool tokenOk(const String& header);

// JSON for the `caps` field.
const char* capsJson();

// POST /api/pair
void handlePair(WebSrv& srv);

}  // namespace pairing
