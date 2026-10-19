# Bastion presence firmware for GeekMagic SmallTV

Reference firmware that turns a GeekMagic SmallTV into a desk display for
Bastion: persona, running sessions, approvals and the companion. It implements
the device API from [`docs/en/presence.md`](../../docs/en/presence.md).

| Board | MCU | Touch | Env | Sessions shown |
|---|---|---|---|---|
| GeekMagic SmallTV Ultra | ESP8266 (ESP-12E), 4 MB | no | `ultra` | 5 |
| GeekMagic SmallTV Pro | ESP32-WROOM-32 | capacitive pad (GPIO32) | `pro` | 8 |

Both use a 240x240 ST7789 panel driven with TFT_eSPI. The Pro display pin
mapping is not verified publicly: see [Pro board status](#pro-board-status).

## What to buy

- A GeekMagic SmallTV Ultra or Pro.
- A 5 V / 1 A USB supply. The USB-C port is power only (no USB-serial).
- For the serial fallback only: a 3.3 V USB-TTL adapter (CH340 or CP2102) and
  four jumper wires. Use 3.3 V logic, never 5 V.

## Build

Requires [PlatformIO](https://platformio.org/).

```bash
cd firmware/smalltv
pio run -e ultra     # .pio/build/ultra/firmware.bin
pio run -e pro       # .pio/build/pro/firmware.bin
```

CI builds both and uploads them as the `smalltv-firmware` artifact.

## First flash

### Plan A: no screwdriver (Ultra)

1. Join the stock firmware's Wi-Fi or LAN and open its web page.
2. Open **Firmware update** and upload `.pio/build/ultra/firmware.bin`.

The stock updater may reject images larger than it expects. This has not been
verified against every stock firmware version; if the upload fails or the
device does not come back, use plan B.

### Plan B: serial (Ultra)

1. Open the case (two screws).
2. Wire the adapter: adapter RX to device TX, adapter TX to device RX, GND to
   GND, 3V3 to 3V3 (pinout in the ESPHome device page, linked below).
3. Hold GPIO0 to GND while powering up to enter the bootloader.
4. `pio run -e ultra -t upload`

### Pro

Use the stock firmware's update page as in plan A, or the serial route if the
board exposes the UART pads (`pio run -e pro -t upload`). Read
[Pro board status](#pro-board-status) first.

## Set up Wi-Fi

On first boot (or after a factory reset) the device opens a Wi-Fi network named
`Bastion-Setup-XXXX` and shows a QR code with the text "Scan to set up". Join
it from a phone (scan the QR code); a captive page lets you pick your network
and enter the password. The device saves it and restarts. If the saved network
is unreachable for 5 minutes, the setup network opens again while the device
keeps retrying.

The device is announced over mDNS as `bastion-xxxx.local`, service
`_bastion-device._tcp` on port 80.

## Pair with Bastion

An unpaired device shows `Pair code NNNN` and its IP. On the computer running
the Bastion daemon:

```bash
bastion presence pair <ip> <code>
bastion presence devices
```

Pairing stores the daemon URL and token on the device (flash only, never
sent anywhere else). The code rotates after each successful pairing; five wrong
codes lock pairing for 60 seconds. One host at a time: pairing again replaces
it. The daemon URL must be plain `http://` (a LAN address).

To show the code again on a paired device: on the Pro, hold the pad for 2
seconds on the main screen; on any board,
`POST /api/pair/open` with `Authorization: Bearer <device token>` shows it for
2 minutes.

## What the screen shows

- Top bar: persona (or "Bastion") and the time (UTC; set `-DBASTION_TZ_MIN=<minutes>`
  to offset it).
- The pet: a helmet-and-visor face whose eyes follow the state (idle, working,
  waiting, error, done) and four bars for water, food, play and rest.
- One row per session with a colored dot (working cyan, waiting amber, done
  green, error red, idle grey) and the tool and detail.
- A full-screen amber card for an approval. On the Pro: tap approves, hold 2 s
  denies (needs `device_approvals = true`). On the Ultra, answer elsewhere.
- A banner for a note, and a "Disconnected" screen after 30 s without a push.
- Pro: tap switches between the sessions view and the big pet view.

## Updating over the air

1. `POST /update/open` with `Authorization: Bearer <device token>`. A 4-digit
   code appears on the screen for 2 minutes.
2. Upload within that window:

```bash
curl -F "firmware=@.pio/build/ultra/firmware.bin" \
  "http://<ip>/update?code=<code>&md5=$(md5sum .pio/build/ultra/firmware.bin | cut -d' ' -f1)"
```

`md5` is optional; when present the image is verified before it is applied.
Five wrong codes close the window.

## Factory reset

Power-cycle the device 6 times in a row, each time within 10 seconds of
power-up. Pairing and Wi-Fi credentials are wiped.

## Memory notes

The Ultra has little RAM: no LVGL, a filtered JSON parse, a 4 KB request cap
(`413` above it) and `503 {"error":"busy"}` when free heap is low. Snapshots
are cut by Bastion to the capabilities the device declares (`max_bytes`,
`max_sessions`).

## Pro board status

Touch is read from GPIO32 with a boot-time baseline (threshold
`BASTION_TOUCH_DELTA`, default 40). The Pro's display pins are NOT verified:
`src/boards/pro.h` uses the Ultra's values as a placeholder and must be
confirmed on hardware before use.

## Credits and third-party notices

Ultra pinout: <https://devices.esphome.io/devices/geekmagic-ultra/>.

THIRD_PARTY:

| Component | License |
|---|---|
| TFT_eSPI | FreeBSD-style (BSD 2-clause style, with parts under other permissive licenses, see the library) |
| ArduinoJson | MIT |
| QRCode (ricmoo) | MIT |
| Arduino core for ESP8266 | LGPL-2.1 |
| Arduino core for ESP32 | Apache-2.0 (parts LGPL-2.1) |

This firmware's source is public, so the LGPL relinking obligations for the
ESP8266 core are met by the source itself: anyone can rebuild the firmware
with a modified core using the commands above.
