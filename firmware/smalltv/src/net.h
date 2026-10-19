// Wi-Fi station, captive setup portal and mDNS.
#pragma once
#include "platform.h"

namespace net {

// Registers the portal routes (/, /save, /rescan, not-found redirect) on
// `srv`, then joins the saved network or opens the portal. Does not block.
void begin(WebSrv& srv);
void loop();

bool connected();
bool portalActive();
const String& deviceId();  // "bastion-4f2a"
const String& apName();    // "Bastion-Setup-4F2A"

}  // namespace net
