#include "render.h"
#include <TFT_eSPI.h>
#include <qrcode.h>

namespace render {

static TFT_eSPI tft;

static constexpr uint16_t rgb(uint8_t r, uint8_t g, uint8_t b) {
  return (uint16_t)(((r & 0xF8) << 8) | ((g & 0xFC) << 3) | (b >> 3));
}

static const uint16_t BG = rgb(10, 14, 20);
static const uint16_t BAR = rgb(28, 34, 42);
static const uint16_t FG = rgb(230, 237, 243);
static const uint16_t DIM = rgb(125, 133, 144);
static const uint16_t TRACK = rgb(38, 44, 53);
static const uint16_t CYAN = rgb(57, 208, 224);
static const uint16_t AMBER = rgb(255, 176, 0);
static const uint16_t GREEN = rgb(63, 185, 80);
static const uint16_t RED = rgb(248, 81, 73);
static const uint16_t GREY = rgb(110, 118, 129);
static const uint16_t HELM = rgb(200, 205, 214);
static const uint16_t VISOR = rgb(13, 17, 23);
static const uint16_t BLACK = rgb(0, 0, 0);
static const uint16_t WHITE = rgb(255, 255, 255);

static const uint16_t BAR_COL[4] = {rgb(57, 208, 224), rgb(255, 138, 60), rgb(255, 121, 198),
                                    rgb(150, 130, 255)};
static const char BAR_LBL[4] = {'W', 'F', 'P', 'R'};

static const int W = 240;
static const int TOP_H = 24;
static const int NOTE_Y = 24, NOTE_H = 20;
static const int PET_Y = 46;
static const int LIST_Y = 116, ROW_H = 24;

// ---- state of what is currently on the glass ------------------------------
static uint8_t lastScreen = SCR_NONE;
static uint8_t lastView = 255;
static uint8_t lastFace = 255;
static uint32_t sigTop, sigNote, sigPet, sigList, sigMain, sigCount;

static void resetSigs() {
  sigTop = sigNote = sigPet = sigList = sigMain = sigCount = 0;
  lastFace = 255;
}

static uint32_t fnv(uint32_t h, const void* p, size_t n) {
  const uint8_t* b = (const uint8_t*)p;
  while (n--) {
    h ^= *b++;
    h *= 16777619u;
  }
  return h;
}
static uint32_t fnvs(uint32_t h, const char* s) { return fnv(h, s, strlen(s)); }
static uint32_t fnvi(uint32_t h, uint32_t v) { return fnv(h, &v, sizeof(v)); }
static const uint32_t SEED = 2166136261u;
static uint32_t nz(uint32_t v) { return v ? v : 1; }

// ---- helpers --------------------------------------------------------------
static void backlight(uint8_t level) {
  uint8_t duty = BL_ACTIVE_LOW ? (uint8_t)(255 - level) : level;
#if defined(ESP8266)
  analogWriteRange(255);
  analogWrite(PIN_TFT_BL, duty);
#else
  ledcSetup(0, 5000, 8);
  ledcAttachPin(PIN_TFT_BL, 0);
  ledcWrite(0, duty);
#endif
}

static void text(const String& s, int x, int y, uint8_t font, uint16_t fg, uint16_t bg,
                 uint8_t datum = TL_DATUM) {
  tft.setTextColor(fg, bg);
  tft.setTextDatum(datum);
  tft.drawString(s, x, y, font);
}

static String fit(const char* s, int maxPx, uint8_t font) {
  String o(s);
  if (tft.textWidth(o, font) <= maxPx) return o;
  while (o.length() > 1 && tft.textWidth(o + "..", font) > maxPx) o.remove(o.length() - 1);
  return o + "..";
}

static uint16_t stColor(uint8_t st) {
  switch (st) {
    case state::ST_WORKING: return CYAN;
    case state::ST_WAITING: return AMBER;
    case state::ST_DONE: return GREEN;
    case state::ST_ERROR: return RED;
    default: return GREY;
  }
}

// Overall pet mood from the snapshot.
static uint8_t petState(const state::Snap& s) {
  if (s.hasApproval) return state::ST_WAITING;
  bool w = false, e = false, k = false, d = false;
  for (uint8_t i = 0; i < s.n; i++) {
    switch (s.sessions[i].st) {
      case state::ST_WAITING: w = true; break;
      case state::ST_ERROR: e = true; break;
      case state::ST_WORKING: k = true; break;
      case state::ST_DONE: d = true; break;
      default: break;
    }
  }
  if (w) return state::ST_WAITING;
  if (e) return state::ST_ERROR;
  if (k) return state::ST_WORKING;
  if (d) return state::ST_DONE;
  return state::ST_IDLE;
}

// ---- pet face (pixel art, drawn with rectangles; 18u x 14u) ---------------
static void thickLine(int x0, int y0, int x1, int y1, int t, uint16_t col) {
  for (int k = 0; k < t; k++) tft.drawLine(x0 + k, y0, x1 + k, y1, col);
}

static void drawFace(int x, int y, int u, uint8_t st, uint8_t frame, bool full) {
  if (full) {
    tft.fillRoundRect(x, y, 16 * u, 14 * u, 2 * u, HELM);
  }
  tft.fillRect(x + 2 * u, y + 4 * u, 12 * u, 7 * u, VISOR);  // visor

  uint16_t col = st == state::ST_WAITING ? AMBER : st == state::ST_ERROR ? RED
               : st == state::ST_DONE ? GREEN : CYAN;
  int lx = x + 4 * u, rx = x + 10 * u, ey = y + 5 * u;

  switch (st) {
    case state::ST_WORKING: {
      static const int8_t dx[4] = {-1, 0, 1, 0};
      int d = dx[frame & 3] * u;
      tft.fillRect(lx + d, ey, 2 * u, 4 * u, col);
      tft.fillRect(rx + d, ey, 2 * u, 4 * u, col);
      break;
    }
    case state::ST_WAITING:  // "!" eyes
      tft.fillRect(lx, ey, 2 * u, 3 * u, col);
      tft.fillRect(lx, ey + 4 * u, 2 * u, u, col);
      tft.fillRect(rx, ey, 2 * u, 3 * u, col);
      tft.fillRect(rx, ey + 4 * u, 2 * u, u, col);
      break;
    case state::ST_ERROR: {  // x eyes
      int s = 3 * u, t = u > 3 ? u / 2 : 2;
      int bx[2] = {x + 3 * u, x + 10 * u};
      for (int i = 0; i < 2; i++) {
        thickLine(bx[i], ey, bx[i] + s, ey + s, t, col);
        thickLine(bx[i] + s - t, ey, bx[i] - t, ey + s, t, col);
      }
      break;
    }
    case state::ST_DONE: {  // ^ ^ eyes
      int t = u > 3 ? u / 2 : 2;
      int bx[2] = {x + 3 * u, x + 9 * u};
      for (int i = 0; i < 2; i++) {
        int by = ey + 2 * u;
        thickLine(bx[i], by + 2 * u, bx[i] + 2 * u, by, t, col);
        thickLine(bx[i] + 2 * u, by, bx[i] + 4 * u - t, by + 2 * u, t, col);
      }
      break;
    }
    default:  // idle
      tft.fillRect(lx, ey, 2 * u, 3 * u, col);
      tft.fillRect(rx, ey, 2 * u, 3 * u, col);
      break;
  }
  if (full && st == state::ST_WAITING) tft.fillRect(x + 16 * u, y + u, 2 * u, 6 * u, HELM);  // raised hand
}

static void drawBars(int x, int y, int w, int h, int gap, uint8_t font, const uint8_t needs[4]) {
  int lblW = font == 2 ? 14 : 10;
  for (int i = 0; i < 4; i++) {
    int by = y + i * (h + gap);
    char l[2] = {BAR_LBL[i], 0};
    tft.setTextSize(1);
    text(l, x, by, 1, DIM, BG);
    int bx = x + lblW, bw = w - lblW;
    tft.fillRect(bx, by, bw, h, TRACK);
    tft.fillRect(bx, by, bw * needs[i] / 100, h, BAR_COL[i]);
  }
}

static void petBlock(const state::Snap& s, uint8_t view) {
  uint8_t ps = petState(s);
  uint8_t frame = (ps == state::ST_WORKING) ? (uint8_t)((millis() / 400) & 3) : 0;
  uint32_t sig = SEED;
  sig = fnvi(sig, ps); sig = fnvi(sig, frame); sig = fnvi(sig, s.level);
  sig = fnv(sig, s.needs, 4); sig = fnvs(sig, s.cue); sig = fnvi(sig, view);
  sig = nz(sig);
  if (sig == sigPet) return;
  sigPet = sig;

  bool full = (lastFace != ps);
  char lv[28];
  if (s.cue[0]) snprintf(lv, sizeof(lv), "Lv %u  wants %s", s.level, s.cue);
  else snprintf(lv, sizeof(lv), "Lv %u", s.level);

  if (view == 0) {
    if (full) tft.fillRect(0, PET_Y, W, LIST_Y - PET_Y - 2, BG);
    drawFace(8, PET_Y + 2, 4, ps, frame, full);
    tft.fillRect(92, PET_Y, W - 92, 16, BG);
    text(fit(lv, W - 96, 2), 94, PET_Y, 2, FG, BG);
    drawBars(94, PET_Y + 22, W - 100, 8, 5, 1, s.needs);
  } else {
    if (full) tft.fillRect(0, PET_Y, W, W - PET_Y, BG);
    drawFace(48, PET_Y + 4, 8, ps, frame, full);
    tft.fillRect(0, 168, W, 18, BG);
    text(fit(lv, W - 8, 2), W / 2, 168, 2, FG, BG, TC_DATUM);
    drawBars(24, 194, W - 48, 8, 4, 2, s.needs);
  }
  lastFace = ps;
}

// ---- main screen ----------------------------------------------------------
static void topBar(const Ctx& c, const state::Snap& s) {
  uint32_t minute = c.epoch ? (c.epoch + BASTION_TZ_MIN * 60UL) / 60 : 0;
  uint32_t sig = nz(fnvi(fnvs(SEED, s.persona), minute));
  if (sig == sigTop) return;
  sigTop = sig;
  tft.fillRect(0, 0, W, TOP_H, BAR);
  text(fit(s.persona[0] ? s.persona : "Bastion", 150, 2), 8, 4, 2, FG, BAR);
  char t[8] = "--:--";
  if (c.epoch) {
    uint32_t m = ((c.epoch + BASTION_TZ_MIN * 60UL) / 60) % 1440;
    snprintf(t, sizeof(t), "%02u:%02u", (unsigned)(m / 60), (unsigned)(m % 60));
  }
  text(t, W - 8, 4, 2, DIM, BAR, TR_DATUM);
}

static void noteBanner(const Ctx& c, const state::Snap& s) {
  bool show = s.hasNote && (s.noteExp == 0 || c.epoch == 0 || c.epoch < s.noteExp);
  uint32_t sig = nz(fnvs(fnvi(SEED, show), show ? s.note : ""));
  if (sig == sigNote) return;
  sigNote = sig;
  tft.fillRect(0, NOTE_Y, W, NOTE_H, show ? AMBER : BG);
  if (show) text(fit(s.note, W - 12, 2), 6, NOTE_Y + 2, 2, BLACK, AMBER);
}

static void sessionList(const state::Snap& s) {
  uint32_t sig = fnvi(fnvi(SEED, s.n), s.more);
  for (uint8_t i = 0; i < s.n; i++) {
    sig = fnvs(fnvs(fnvs(sig, s.sessions[i].name), s.sessions[i].tool), s.sessions[i].det);
    sig = fnvi(sig, s.sessions[i].st);
  }
  sig = nz(sig);
  if (sig == sigList) return;
  sigList = sig;

  int rows = (W - LIST_Y) / ROW_H;  // 5
  tft.fillRect(0, LIST_Y, W, W - LIST_Y, BG);
  if (s.n == 0) {
    text("No active sessions", W / 2, LIST_Y + 30, 2, DIM, BG, TC_DATUM);
    return;
  }
  bool overflow = s.n > rows || s.more > 0;
  int shown = s.n;
  if (overflow && shown > rows - 1) shown = rows - 1;
  for (int i = 0; i < shown; i++) {
    const state::Session& ss = s.sessions[i];
    int y = LIST_Y + i * ROW_H;
    tft.fillCircle(14, y + 12, 5, stColor(ss.st));
    text(fit(ss.name[0] ? ss.name : "session", 92, 2), 26, y + 4, 2, FG, BG);
    String d = ss.tool;
    if (ss.det[0]) d += d.length() ? String(" ") + ss.det : String(ss.det);
    text(fit(d.c_str(), W - 124 - 4, 2), 124, y + 4, 2, DIM, BG);
  }
  if (overflow) {
    int hidden = (int)s.n - shown + (int)s.more;
    char m[16];
    snprintf(m, sizeof(m), "+%d more", hidden);
    text(m, 26, LIST_Y + shown * ROW_H + 4, 2, DIM, BG);
  }
}

static void drawMain(const Ctx& c, const state::Snap& s) {
  topBar(c, s);
  noteBanner(c, s);
  petBlock(s, c.view);
  if (c.view == 0) sessionList(s);
}

// ---- approval card --------------------------------------------------------
static void drawApproval(const Ctx& c, const state::Snap& s) {
  uint32_t sig = nz(fnvs(fnvs(fnvs(SEED, s.apId), s.apTool), s.apDet));
  if (sig != sigMain) {
    sigMain = sig;
    sigCount = 0;
    tft.fillScreen(AMBER);
    text("Approve?", W / 2, 14, 4, BLACK, AMBER, TC_DATUM);
    text(fit(s.apTool[0] ? s.apTool : "tool", W - 16, 4), W / 2, 56, 4, BLACK, AMBER, TC_DATUM);
    // wrap the detail into up to 4 lines
    String rest = s.apDet;
    for (int line = 0; line < 4 && rest.length(); line++) {
      int n = rest.length();
      while (n > 1 && tft.textWidth(rest.substring(0, n), 2) > W - 16) n--;
      text(rest.substring(0, n), 8, 100 + line * 20, 2, BLACK, AMBER);
      rest = rest.substring(n);
    }
    text(BASTION_TOUCH ? "tap = approve, hold 2s = deny" : "Answer in the Bastion app", W / 2,
         W - 24, 2, BLACK, AMBER, TC_DATUM);
  }
  uint32_t cs = nz((uint32_t)(c.approvalRemain + 2));
  if (cs != sigCount) {
    sigCount = cs;
    tft.fillRect(W - 60, 0, 60, 20, AMBER);
    if (c.approvalRemain >= 0) {
      char r[8];
      snprintf(r, sizeof(r), "%ds", (int)c.approvalRemain);
      text(r, W - 8, 4, 2, BLACK, AMBER, TR_DATUM);
    }
  }
}

// ---- code screens ---------------------------------------------------------
static void bigCode(const char* title, uint16_t code, int y) {
  text(title, W / 2, y, 4, FG, BG, TC_DATUM);
  char b[8];
  snprintf(b, sizeof(b), "%04u", (unsigned)code);
  tft.setTextSize(2);
  text(b, W / 2, y + 40, 4, AMBER, BG, TC_DATUM);
  tft.setTextSize(1);
}

static void drawPair(const Ctx& c) {
  uint32_t sig = nz(fnvi(fnvs(fnvs(SEED, c.ip), c.id), c.pairCode));
  if (sig == sigMain) return;
  sigMain = sig;
  tft.fillScreen(BG);
  bigCode("Pair code", c.pairCode, 24);
  text("On your computer run:", W / 2, 136, 2, DIM, BG, TC_DATUM);
  text("bastion presence pair", W / 2, 158, 2, FG, BG, TC_DATUM);
  char l[32];
  snprintf(l, sizeof(l), "%s %04u", c.ip, (unsigned)c.pairCode);
  text(l, W / 2, 178, 2, CYAN, BG, TC_DATUM);
  text(c.id, W / 2, 214, 2, DIM, BG, TC_DATUM);
}

static void drawUpdate(const Ctx& c) {
  uint32_t sig = nz(fnvi(SEED, c.updateCode));
  if (sig == sigMain) return;
  sigMain = sig;
  tft.fillScreen(BG);
  bigCode("Update code", c.updateCode, 24);
  text("Upload within 2 minutes", W / 2, 150, 2, DIM, BG, TC_DATUM);
  text(c.ip, W / 2, 176, 2, FG, BG, TC_DATUM);
}

static void drawQr(const char* data, int top, int scale) {
  static uint8_t buf[160];  // QR version 3 needs ~106 bytes
  QRCode qr;
  qrcode_initText(&qr, buf, 3, ECC_LOW, data);
  int size = qr.size * scale;
  int x0 = (W - size) / 2;
  tft.fillRect(x0 - 2 * scale, top - 2 * scale, size + 4 * scale, size + 4 * scale, WHITE);
  for (int y = 0; y < qr.size; y++)
    for (int x = 0; x < qr.size; x++)
      if (qrcode_getModule(&qr, x, y)) tft.fillRect(x0 + x * scale, top + y * scale, scale, scale, BLACK);
}

static void drawSetup(const Ctx& c) {
  uint32_t sig = nz(fnvs(SEED, c.ap));
  if (sig == sigMain) return;
  sigMain = sig;
  tft.fillScreen(BG);
  char q[48];
  snprintf(q, sizeof(q), "WIFI:T:nopass;S:%s;;", c.ap);
  drawQr(q, 20, 5);
  text("Scan to set up", W / 2, 180, 4, FG, BG, TC_DATUM);
  text(String("Wi-Fi: ") + c.ap, W / 2, 214, 2, DIM, BG, TC_DATUM);
}

static void drawDisconnected(const Ctx& c) {
  uint32_t sig = nz(fnvi(fnvi(fnvs(fnvs(SEED, c.ip), c.id), c.wifiUp), c.paired));
  if (sig == sigMain) return;
  sigMain = sig;
  tft.fillScreen(BG);
  drawFace(48, 20, 8, state::ST_IDLE, 0, true);
  tft.fillRect(48 + 2 * 8, 20 + 4 * 8, 12 * 8, 7 * 8, VISOR);  // eyes closed
  tft.fillRect(48 + 4 * 8, 20 + 7 * 8, 2 * 8, 4, GREY);
  tft.fillRect(48 + 10 * 8, 20 + 7 * 8, 2 * 8, 4, GREY);
  text(c.wifiUp ? "Disconnected" : "No Wi-Fi", W / 2, 150, 4, FG, BG, TC_DATUM);
  text(c.wifiUp ? "No update from Bastion" : "Reconnecting...", W / 2, 184, 2, DIM, BG, TC_DATUM);
  text(c.wifiUp ? String(c.id) + "  " + c.ip : String(c.id), W / 2, 210, 2, GREY, BG, TC_DATUM);
}

// ---- public ---------------------------------------------------------------
void begin() {
  tft.init();
  tft.setRotation(0);
  tft.fillScreen(BG);
  backlight(220);
}

void message(const char* l1, const char* l2) {
  tft.fillScreen(BG);
  text(l1, W / 2, 100, 4, FG, BG, TC_DATUM);
  if (l2) text(l2, W / 2, 140, 2, DIM, BG, TC_DATUM);
  lastScreen = SCR_NONE;
}

void draw(const Ctx& c, const state::Snap& s) {
  if (c.screen != lastScreen || (c.screen == SCR_MAIN && c.view != lastView)) {
    tft.fillScreen(c.screen == SCR_APPROVAL ? AMBER : BG);
    resetSigs();
    lastScreen = c.screen;
    lastView = c.view;
  }
  switch (c.screen) {
    case SCR_SETUP: drawSetup(c); break;
    case SCR_PAIR: drawPair(c); break;
    case SCR_UPDATE: drawUpdate(c); break;
    case SCR_DISCONNECTED: drawDisconnected(c); break;
    case SCR_APPROVAL: drawApproval(c, s); break;
    default: drawMain(c, s); break;
  }
}

}  // namespace render
