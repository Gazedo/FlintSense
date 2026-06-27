# FlintMesh — Wildfire Early Warning Sensor Network

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
│   [Sensor Node] ──LoRa──▶ [Bridge Node] ──LoRa──▶ [Gateway] │
│   (flint-node)           (flint-bridge)         (flint-   │
│                                                    gateway) │
└─────────────────────────────────────────────────────────────┘
         │                                        │
         │  Embassy async / no_std                │  std / Tokio
         │  ESP32                                 │  Raspberry Pi
                                                  │
                                           InfluxDB + Grafana
```

Mesh routing uses flood routing with hop-limit deduplication — the same
semantics as [Meshtastic](https://meshtastic.org/).  All packet types and
routing logic live in the shared `flint-proto` crate so embedded nodes and
the gateway always agree on the wire format.

---

## Crates

| Crate | Target | Description |
|---|---|---|
| [`flint-proto`](./flint-proto) | `no_std` (any) | Shared packet types, mesh envelope, seen-packet cache, postcard encode/decode |
| [`flint-bridge`](./flint-bridge) | `xtensa-esp32-none-elf` | LoRa RX + transparent mesh relay + USB binary bridge on Heltec WiFi LoRa 32 V2 |
| [`flint-node`](./flint-node) | `xtensa-esp32-none-elf` | Sensor node firmware on Heltec WiFi LoRa 32 V2 (TX test → real sensors in progress) |
| `flint-gateway` | host (RPi) | Serial reader/decoder for the bridge stream (InfluxDB + Grafana integration planned) |

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

### Development (Phase 0) — current
- **Heltec WiFi LoRa 32 V2** — ESP32 + SX1276 + OLED, used for all firmware
  development.  Two boards: one running `flint-node` (TX), one running
  `flint-bridge` (RX + relay + USB bridge).

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
cargo install espflash
```

Add to `~/.zshrc` (or source before each session):
```bash
. $HOME/export-esp.sh
```

Mac: install the [CP2102 USB driver](https://www.silabs.com/developers/usb-to-uart-bridge-vcp-drivers)
before connecting any board.

### Build

```bash
# Build all crates
cargo build --workspace

# Or a single crate
cargo build -p flint-node
cd flint-bridge && cargo build
```

### Flash & Monitor

Set your USB port in `.cargo/config.toml` (workspace root), then:

```bash
# Flash and open serial monitor
cd flint-node   && cargo run --release
cd flint-bridge && cargo run --release
```

Log output streams over UART via `espflash --monitor`.

### Documentation

```bash
cargo doc --workspace --no-deps --open
```

---

## Bring-up Sequence

| Step | Status | Description |
|---|---|---|
| 1 | ✅ | **Arduino loopback** — confirmed RF link on two Heltec V2 boards |
| 2 | ✅ | **Rust RX ↔ Arduino TX** — `flint-bridge` receiving from factory firmware, validated `lora-phy` config |
| 3 | ✅ | **Full Rust both sides** — `flint-node` TX and `flint-bridge` RX passing `MeshEnvelope` packets end-to-end |
| 4 | 🔄 | **Sensor integration** — fuel moisture (ADC resistive dowel), SHT40 (I2C), MAX17048 (I2C) |
| 5 | ⬜ | **Anemometer pulse counting** — GPIO interrupt task |
| 6 | ⬜ | **RPi gateway** — `flint-gateway` receiving packets, writing to InfluxDB |
| 7 | ⬜ | **SDR passive monitor** — `gr-lora_sdr` chirp waterfall for demos and debugging |

---

## Packet Format

Wire format is [postcard](https://github.com/jamesmunns/postcard)-encoded
`MeshEnvelope<FlintPayload>`, approximately 20–24 bytes per sensor reading.

```
MeshEnvelope {
    from:       u32   // node ID (lower 4 bytes of MAC)
    to:         u32   // 0xFFFFFFFF = broadcast
    packet_id:  u32   // random, used for dedup
    hop_limit:  u8    // decremented on each relay (default 3)
    hop_start:  u8    // original hop_limit
    payload:    FlintPayload::SensorReading {
        node_id:       u8
        temp_c:        i16   // °C × 100
        humidity_pct:  u8
        wind_speed_ms: u8    // m/s × 2
        wind_dir_deg:  u16
        fuel_moisture: u8    // 0–100 %, resistive wood dowel
        battery_soc:   u8
        battery_mv:    u16
        sequence:      u16
    }
}
```

---

## License

MIT OR Apache-2.0
