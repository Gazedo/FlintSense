# KindleSense — Wildfire Early Warning Sensor Network

A distributed, solar-powered LoRa sensor network that measures fire-weather
conditions in real time.  Nodes report data compatible with the
[Fire Weather Index (FWI)](https://cwfis.cfs.nrcan.gc.ca/background/summary/fwi) and
[NFDRS](https://www.wfas.net/index.php/nfdrs-fire-danger-model-fire-danger-8) frameworks
used by land managers and fire agencies.

Built in Rust — `no_std` Embassy async firmware on ESP32 nodes, `std` gateway on
Raspberry Pi.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        LoRa Mesh                            │
│                                                             │
│   [Sensor Node] ──LoRa──▶ [Relay Node] ──LoRa──▶ [Gateway] │
│   (kindle-node)           (kindle-debug)          (kindle-  │
│                                                    gateway) │
└─────────────────────────────────────────────────────────────┘
         │                                        │
         │  Embassy async / no_std                │  std / Tokio
         │  ESP32 / ESP32-C3                      │  Raspberry Pi
                                                  │
                                           InfluxDB + Grafana
```

Mesh routing uses flood routing with hop-limit deduplication — the same
semantics as [Meshtastic](https://meshtastic.org/).  All packet types and
routing logic live in the shared `kindle-proto` crate so embedded nodes and
the gateway always agree on the wire format.

---

## Crates

| Crate | Target | Description |
|---|---|---|
| [`kindle-proto`](./kindle-proto) | `no_std` (any) | Shared packet types, mesh envelope, seen-packet cache, postcard encode/decode |
| [`kindle-debug`](./kindle-debug) | `xtensa-esp32-none-elf` | LoRa RX + transparent mesh relay on Heltec WiFi LoRa 32 V2 |
| `kindle-node` *(planned)* | `riscv32imc-unknown-none-elf` | Full sensor node firmware for M5Stack Stamp-C3U |
| `kindle-gateway` *(planned)* | host (RPi) | Packet receiver, InfluxDB writer, Grafana integration |

---

## Sensor Measurements

| Measurement | Sensor | Interface |
|---|---|---|
| Air temperature + relative humidity | SHT40 | I2C 0x44 |
| Barometric pressure | BME280 | I2C |
| Wind speed + direction | Anemometer + vane | GPIO pulse count |
| Dead fuel moisture proxy | Wood dowel + resistive probes | ADC |
| Battery state-of-charge | MAX17048 fuel gauge | I2C 0x36 |

---

## Hardware

### Development (Phase 0)
- **Heltec WiFi LoRa 32 V2** — ESP32 + SX1276 + OLED, used for firmware development and as the debug/relay node

### Production Node (Phase 1)
- **M5Stack Stamp-C3U** (ESP32-C3, ~5 µA deep sleep) + harvested SX1276
- Custom KiCad PCB: Stamp-C3U + SX1276 + MAX17048 + TP4056 + solar input + sensor headers
- Fabricated at JLCPCB/PCBWay
- 3D-printed weatherproof enclosure

### Gateway
- Raspberry Pi + Heltec V2 as LoRa receiver
- InfluxDB / TimescaleDB for time-series storage
- Grafana dashboard (public)

---

## Getting Started

### Prerequisites

```bash
cargo install espup && espup install   # Xtensa toolchain + export-esp.sh
cargo install ldproxy                  # Required linker proxy
cargo install espflash
```

Add to `~/.zshrc` (Mac):
```bash
. $HOME/export-esp.sh
```

Mac: install the [CP2102 USB driver](https://www.silabs.com/developers/usb-to-uart-bridge-vcp-drivers) before connecting any board.

### Build

```bash
# Check all crates compile (host target — for kindle-proto only)
cargo check -p kindle-proto

# Build debug receiver firmware (requires Xtensa toolchain)
cd kindle-debug
cargo build --release
```

### Documentation

```bash
cargo doc --workspace --no-deps --open
```

### Flash & Monitor

```bash
cd kindle-debug
# Edit .cargo/config.toml to set your actual USB port, then:
cargo run --release
```

Logs stream over RTT via `espflash --monitor`.  Received LoRa packets are
decoded and printed; unknown-format packets are hex-dumped for debugging
against the Arduino default firmware.

### Generate Documentation

```bash
cargo doc --workspace --no-deps --open
```

---

## Bring-up Sequence

1. **Arduino loopback** — Flash Heltec Arduino TX/RX examples on two V2 boards.
   Confirm RF link.  Record exact SF / BW / frequency / sync word — these become
   the Rust firmware targets.
2. **Rust RX ↔ Arduino TX** — `kindle-debug` receiving from Arduino TX proves
   `lora-phy` driver config is correct.
3. **Full Rust both sides** — Embassy firmware on both boards.
4. **Add I2C sensors** — SHT40 first, then MAX17048.
5. **Anemometer pulse counting** — GPIO interrupt task.
6. **RPi gateway** — `kindle-gateway` receiving packets, writing to InfluxDB.
7. **SDR passive monitor** — `gr-lora_sdr` chirp waterfall for demos and debugging.

---

## Packet Format

Wire format is [postcard](https://github.com/jamesmunns/postcard)-encoded
`MeshEnvelope<KindlePayload>`, approximately 20–24 bytes per sensor reading.

```
MeshEnvelope {
    from:       u32   // node ID (lower 4 bytes of MAC)
    to:         u32   // 0xFFFFFFFF = broadcast
    packet_id:  u32   // random, used for dedup
    hop_limit:  u8    // decremented on each relay (default 3)
    hop_start:  u8    // original hop_limit
    payload:    KindlePayload::SensorReading {
        node_id:       u8
        temp_c:        i16   // °C × 100
        humidity_pct:  u8
        wind_speed_ms: u8    // m/s × 2
        wind_dir_deg:  u16
        fuel_moisture: u8
        battery_soc:   u8
        battery_mv:    u16
        sequence:      u16
    }
}
```

---

## License

MIT OR Apache-2.0
