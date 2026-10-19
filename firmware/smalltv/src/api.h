// Device HTTP API: /api/info, /api/pair, /api/state, /update/open.
#pragma once
#include "platform.h"

namespace api {

// Registers routes on `srv`. Call before srv.begin().
void begin(WebSrv& srv);

}  // namespace api
