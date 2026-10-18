#include "actions.h"
#include "pairing.h"
#include "net.h"

namespace actions {

bool send(const char* action, const char* id) {
  if (!pairing::paired() || !net::connected()) return false;
  const store::Paired& p = pairing::cfg();

  // Plain HTTP only: the daemon is reached over the local network.
  WiFiClient client;
  HTTPClient http;
  http.setTimeout(3000);
  if (!http.begin(client, p.bastionUrl + "/presence/action")) return false;
  http.addHeader("Content-Type", "application/json");
  http.addHeader("x-bastion-device-token", p.bastionToken);
  String body = String("{\"action\":\"") + action + "\",\"id\":\"" + id + "\"}";
  int code = http.POST(body);
  http.end();
  return code >= 200 && code < 300;
}

}  // namespace actions
