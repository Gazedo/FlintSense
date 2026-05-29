#!/usr/bin/env bash
# Flash flint-node to RAK4631 via adafruit-nrfutil DFU over USB serial.
# Prerequisites:
#   rustup component add llvm-tools
#   cargo install cargo-binutils
#   pip3 install adafruit-nrfutil
# Double-tap reset on the board before running to enter bootloader mode.
set -e

ELF="$1"
PORT="${FLASH_PORT:-/dev/tty.usbmodem101}"
HEX=$(mktemp /tmp/flint-node-XXXXXX.hex)
PKG=$(mktemp /tmp/flint-node-XXXXXX.zip)

trap 'rm -f "$HEX" "$PKG"' EXIT

# Convert ELF → Intel HEX (nrfutil requires HEX format)
rust-objcopy -O ihex "$ELF" "$HEX"

adafruit-nrfutil dfu genpkg \
    --dev-type 0x0052 \
    --application "$HEX" \
    "$PKG"

adafruit-nrfutil dfu serial \
    --package "$PKG" \
    --port "$PORT" \
    --baudrate 115200
