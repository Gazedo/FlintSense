## Why

The `flint-debug` crate was built to *watch* the mesh during bring-up — it logs
received packets as human-readable text over USB serial. That is the wrong shape
for the next step: feeding live mesh traffic into the host-side gateway as data.
Human-formatted log lines have no delimiters, no integrity check, and interleave
with framework/panic noise, so they cannot be parsed reliably. The crate's role
is changing from *debugger* to *bridge*: receive LoRa frames and report them to
the host in a stable binary form that the gateway can consume — and that a raw
LoRa radio attached directly to the gateway could later produce with the same
decode path.

## What Changes

- **Rename** the `flint-debug` crate to **`flint-bridge`** to reflect its new
  role (LoRa → host bridge). The Heltec V2 receiver/relay behaviour is retained;
  the *output* changes. A fresh `flint-debug` can be written later if a pure
  watch tool is wanted again.
- Add a **binary bridge protocol** in `flint-proto`: a self-delimiting framed
  record with a sync word, a **type tag**, a length, a body, and a CRC trailer so
  a host can locate records and reject corruption. Two record types:
  - `FrameReport` (tag): link quality (`rssi`/`snr`/`len`), disposition
    (relayed / local-only / duplicate), and the decoded envelope bytes.
  - `RawFrame` (tag): link quality plus the raw received bytes for packets that
    do not decode as a `MeshEnvelope`.
- Add a **host→device command channel**: the bridge listens on USB serial for a
  single command byte. `b` selects **binary mode** (default), `d` selects
  **debug mode**.
- **Default to binary, silence logs:** in binary mode the firmware sets the
  runtime log level to off so `info!`/`trace!` cannot pollute the binary stream.
  In debug mode it raises the level to trace and emits human-readable frame text
  *in place of* the binary stream — for debugging the bridge itself.
- Add a **reader to `flint-gateway`**: open the serial port, decode the binary
  bridge stream with `flint-proto`, and pretty-print frames. This becomes the
  gateway's first real feature and the host-side decoder for bring-up.

## Capabilities

### New Capabilities
- `bridge-frame-protocol`: The binary, self-delimiting record format
  (`flint-proto`) that `flint-bridge` emits per reception, including the decoded
  and raw record types and the framing/integrity rules a host relies on.
- `bridge-control`: The host→device command-byte channel and the output-mode /
  log-level semantics it governs (binary vs. debug).

### Modified Capabilities
<!-- No existing specs in openspec/specs/; nothing to modify. -->

## Impact

- **Code**: `flint-debug/` → `flint-bridge/` (crate rename: dir, `Cargo.toml`,
  bin name, README); `flint-bridge/src/main.rs` (emit binary records, add UART
  command-RX task, runtime log gating); `flint-proto/src/lib.rs` (new bridge
  record types + framing/CRC encode + host decode + `Display`); `flint-gateway`
  (serial reader/pretty-printer); `flint-bridge/.cargo/config.toml`
  (`ESP_LOG = "TRACE"`).
- **Consumers**: `flint-gateway` gains a working ingest path; the same decode
  path can later accept bytes from a radio wired directly to the gateway.
- **Dependencies**: `flint-gateway` adds a host serial crate (e.g.
  `serialport`); `flint-proto` gains no new *required* deps (no_std, IO-free).
- **Hardware/flashing**: no pin or modem-config changes; same `espflash` flow.
