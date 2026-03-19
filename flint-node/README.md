# flint-node

LoRa transmit test firmware for the **Heltec WiFi LoRa 32 V2** (ESP32 + SX1276).

Sends a `MeshEnvelope` containing a fake `WeatherPacket` every 5 seconds.
Use alongside `flint-debug` on a second board to verify the full TX → RX → decode
path before connecting real sensors.

## Hardware

Heltec WiFi LoRa 32 V2 pin mapping:

| Signal | GPIO |
|---|---|
| SPI SCK | 5 |
| SPI MISO | 19 |
| SPI MOSI | 27 |
| SX1276 CS | 18 |
| SX1276 RST | 14 |
| SX1276 DIO0 | 26 |
| OLED SDA | 4 |
| OLED SCL | 15 |
| OLED RST | 16 |

## Configuration

Set your USB port in `.cargo/config.toml`:

```toml
[target.xtensa-esp32-none-elf]
runner = "espflash flash --monitor --baud 921600 --port /dev/cu.usbserial-XXXX"
```

LoRa modem settings (`LORA_FREQUENCY_HZ`, `LORA_SF`, `LORA_BW`, `LORA_CR`) are
constants at the top of `src/main.rs` and **must match** the values in
`flint-debug` exactly.

`NODE_ADDR` and `NODE_ID` are hardcoded test values. When real sensor hardware
is added these will be derived from the chip MAC address.

## Build & Flash

```bash
# From the flint-node directory
cargo run --release
```

Requires the Xtensa ESP32 Rust toolchain (`espup install`).
The `rust-toolchain.toml` in the workspace root pins the correct toolchain
automatically.

## OLED output

```
FlintNode TX
TX: 3
```

Counter increments after each successful transmission.

## Serial output

```
INFO flint-node booting
INFO TX loop — 915000000Hz  SF7  BW125KHz  CR4_5  5000ms interval
INFO TX seq=0 (N bytes)
INFO TX seq=1 (N bytes)
...
```
