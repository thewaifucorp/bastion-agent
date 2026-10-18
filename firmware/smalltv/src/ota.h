// Code-gated firmware upload: POST /update/open, then POST /update?code=NNNN.
#pragma once
#include "platform.h"

namespace ota {

void begin(WebSrv& srv);
void loop();

// POST /update/open (caller has already checked the bearer token).
void handleOpen(WebSrv& srv);

bool codeVisible();
uint16_t code();

}  // namespace ota
