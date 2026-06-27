## 1. Rename crate to flint-bridge

- [x] 1.1 Rename the `flint-debug/` directory to `flint-bridge/`
- [x] 1.2 Update `Cargo.toml`: package `name`, `[[bin]]` name/path, and the
  `description` to reflect the LoRa→host bridge role
- [x] 1.3 Update `README.md` title/description from debug firmware to bridge
- [x] 1.4 Grep the repo for `flint-debug` references (root docs, `Makefile`,
  README) and update them
- [x] 1.5 Confirm the renamed crate still builds (`cargo check` from the crate
  dir on the `xtensa` toolchain passes; flashing covered by 5.2)

## 2. Bridge protocol in flint-proto

- [x] 2.1 Define a `Disposition` enum (relayed / local-only / duplicate)
- [x] 2.2 Define the link-quality header (`rssi`, `snr`, `len`) shared by both
  record types
- [x] 2.3 Define the two record types (`FrameReport`, `RawFrame`) with distinct
  type tags
- [x] 2.4 Implement framing: `[sync][tag][len][body][CRC]` — pick a sync word and
  CRC variant (e.g. CRC-16/CCITT) as constants
- [x] 2.5 Implement device-side `encode` of each record into a caller buffer
  (no_std, no alloc); `FrameReport` body embeds the encoded envelope bytes
- [x] 2.6 Implement host-side `decode`: scan to sync word, validate length + CRC,
  return the record or a recoverable framing error; skip non-record bytes
- [x] 2.7 Add `Display` (or a format helper) for human-readable rendering of each
  record, reused by both the gateway and the bridge's debug mode
- [x] 2.8 Add a round-trip unit test (encode → decode) and a resync test
  (garbage bytes before a valid record)

## 3. Bridge firmware output + control

- [x] 3.1 Set `ESP_LOG = "TRACE"` in `flint-bridge/.cargo/config.toml`
- [x] 3.2 Add an `OutputMode` state (Binary default / Debug) shared with the RX
  loop
- [x] 3.3 At boot, set mode Binary and call `log::set_max_level(Off)`
- [x] 3.4 In the RX loop, on a decoded `MeshEnvelope` emit a `FrameReport` with
  the correct `Disposition` (relayed / local-only / duplicate)
- [x] 3.5 In the RX loop, on an undecodable packet emit a `RawFrame`
- [x] 3.6 In Debug mode, render records as human-readable text instead of binary
  and ensure trace logs are enabled
- [x] 3.7 Add a UART RX task that reads command bytes: `b` → Binary +
  `set_max_level(Off)`, `d` → Debug + `set_max_level(Trace)`, other bytes ignored
- [x] 3.8 Verify commands are idempotent and frame reception continues while
  listening for commands (idempotent by construction — set-mode bytes + concurrent
  embassy task; runtime behaviour validated on hardware)

## 4. Gateway reader

- [x] 4.1 Add a host serial dependency (e.g. `serialport`) to `flint-gateway`
- [x] 4.2 Open the serial port and feed bytes through the `flint-proto` decoder,
  skipping non-record bytes
- [x] 4.3 Pretty-print each `FrameReport`/`RawFrame` using the shared `Display`
  helper, including link quality and disposition
- [x] 4.4 Send `b` on connect to assert binary mode
- [x] 4.5 Manually verify end-to-end: bridge → USB → gateway decodes a live
  `SensorReading`, and a `RawFrame` for noise/Arduino packets (validated on hardware)

## 5. Verification

- [x] 5.1 Run `flint-proto` codec tests (round-trip + resync)
- [x] 5.2 Flash the renamed `flint-bridge`, confirm binary-by-default stream is
  clean (no log text) and `d`/`b` toggles human/binary with trace logs
  (validated on hardware)
- [x] 5.3 Update READMEs/docs describing the bridge protocol and the gateway
  reader usage
