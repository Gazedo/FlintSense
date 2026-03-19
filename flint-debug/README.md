# flint-debug

LoRa receive + transparent mesh relay firmware for the
**Heltec WiFi LoRa 32 V2** (ESP32 + SX1276).

This is the first firmware target in the FlintSense bring-up sequence.
It serves two purposes:

1. **Debug receiver** — decodes incoming `MeshEnvelope` packets and logs them
   over RTT/USB serial.  Unknown-format packets (e.g. from the Arduino default
   sketch) are hex-dumped so you can inspect raw bytes while validating the
   `lora-phy` driver config.

2. **Mesh relay** — if a received packet has `hop_limit > 0` and its
   `packet_id` is not in the seen-packet cache, the node decrements `hop_limit`
   and rebroadcasts — extending mesh coverage without any dedicated relay
   hardware.

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
| SX1276 DIO1 | 35 |

## Configuration

Before flashing, set your USB port in `.cargo/config.toml`:

```toml
[target.xtensa-esp32-none-elf]
runner = "espflash flash --monitor --baud 921600 --port /dev/cu.usbserial-XXXX"
```

LoRa modem config (SF / BW / CR / frequency / sync word) is set in
`src/main.rs` inside `rx_task`.  Match these values to your transmitter —
record them from the Arduino TX sketch during the loopback test.

## Build & Flash

```bash
# From workspace root — requires Xtensa toolchain (espup install)
cd flint-debug
cargo run --release
```

Logs are visible in the `espflash --monitor` output.

## Task Structure

```
main
 ├── rx_task    — owns the SX1276 radio; listens continuously and forwards
 │               raw packet bytes to RX_CHANNEL
 └── relay_task — decodes MeshEnvelope from channel, logs payload via defmt,
                  rebroadcasts if hop_limit > 0 and packet_id is unseen
```
