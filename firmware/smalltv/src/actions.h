// Device -> Bastion: POST {bastion_url}/presence/action.
#pragma once
#include "platform.h"

namespace actions {

// action: "approve" | "deny". Returns true on a 2xx answer.
bool send(const char* action, const char* id);

}  // namespace actions
