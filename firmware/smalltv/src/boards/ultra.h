// GeekMagic SmallTV Ultra: ESP8266 (ESP-12E), ST7789V 240x240, no touch.
// Pinout source: https://devices.esphome.io/devices/geekmagic-ultra/
// Plain #defines only: this header is force-included (-include) in every
// translation unit, including the C sources of the Arduino core.
#pragma once

#define BASTION_BOARD "ultra"
#define BASTION_TOUCH 0
#define BASTION_MAX_BYTES 4096
#define BASTION_MAX_SESSIONS 5
#define BASTION_MIN_HEAP 14000

#define TFT_MOSI 13
#define TFT_SCLK 14
#define TFT_DC 0
#define TFT_RST 2

#define PIN_TFT_BL 5
#define BL_ACTIVE_LOW 1
