# flint-bridge

LoRa receive + transparent mesh relay + **USB bridge** firmware for the
**Heltec WiFi LoRa 32 V2** (ESP32 + SX1276).

`flint-bridge` receives `MeshEnvelope` packets off the air, relays unseen ones,
and **reports every reception to the host over USB as binary records** that
`flint-gateway` decodes with the shared `flint-proto` definitions. The same
record format is what a LoRa radio wired directly into the gateway would emit, so
the host decode path is source-agnostic.

(The crate was formerly `flint-debug`, a human-readable watch tool. Its watch
behaviour now lives in the on-demand **debug mode** below; a standalone watch
tool can be re-created later if wanted.)

## Output modes

The bridge starts in **binary mode** and accepts single-byte commands on USB
serial:

| Command | Mode | Behaviour |
|---|---|---|
| `b` | Binary (default) | Logs suppressed (`set_max_level(Off)`); emits binary `FrameReport`/`RawFrame` records. Clean machine-parseable stream. |
| `d` | Debug | Raises logs to trace; renders frames as human-readable text *instead of* binary — for debugging the bridge itself. |

Commands are idempotent, so a host can send `b` on connect to force a known
state. Unknown bytes are ignored.

## Wire format

Records are defined in `flint-proto` (`bridge` module): a self-delimiting frame
`[sync][tag][len][body][CRC-16]`. `FrameReport` (tag `0x01`) carries link quality
(`rssi`/`snr`/`len`), the receiver's disposition (relayed / local-only /
duplicate), and the received envelope bytes. `RawFrame` (tag `0x02`) carries link
quality plus the raw bytes of any packet that did not decode as a `MeshEnvelope`.
The sync word + length + CRC let the host skip stray boot/panic text and resync.

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

`ESP_LOG = "TRACE"` is set in `.cargo/config.toml` so debug mode can reach trace
level; the runtime gate (`log::set_max_level`) keeps logs off in binary mode.

LoRa modem config (SF / BW / CR / frequency / sync word) is set in `src/main.rs`.
Match these values to your transmitter.

## Build & Flash

```bash
# From workspace root — requires Xtensa toolchain (espup install)
cd flint-bridge
cargo run --release
```

To read the binary stream on the host, run the gateway against the same port:

```bash
cd flint-gateway
cargo run -- /dev/cu.usbserial-XXXX
```

Or send `d` in a serial monitor to eyeball frames directly from the bridge.
