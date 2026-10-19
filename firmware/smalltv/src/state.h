// Latest presence snapshot, reduced to fixed-size fields.
#pragma once
#include "platform.h"

namespace state {

enum St : uint8_t { ST_IDLE = 0, ST_WORKING, ST_WAITING, ST_DONE, ST_ERROR };

struct Session {
  char name[20];
  char tool[12];
  char det[34];
  uint8_t st;
};

struct Snap {
  uint32_t seq;
  uint32_t now;  // Unix seconds at the host when the snapshot was built
  char persona[20];
  uint8_t level;
  uint8_t needs[4];  // water, food, play, rest (0..100)
  char cue[8];
  Session sessions[BASTION_MAX_SESSIONS];
  uint8_t n;
  uint16_t more;  // sessions not shown (dropped by Bastion or by this device)
  bool hasApproval;
  char apId[16];
  char apTool[20];
  char apDet[50];
  uint32_t apExp;
  bool hasNote;
  char note[50];
  uint32_t noteExp;
};

// Parses a snapshot JSON body. Returns false on malformed input.
bool parse(const char* body, size_t len, Snap& out);

void set(const Snap& s);
const Snap& get();
bool everReceived();
uint32_t ageMs();
bool fresh();         // received a push within BASTION_STALE_MS
uint32_t epochNow();  // snapshot time advanced by the local clock; 0 if unknown

}  // namespace state
