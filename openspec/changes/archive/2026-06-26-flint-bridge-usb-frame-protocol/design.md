## Context

`flint-debug` runs on the Heltec WiFi LoRa 32 V2 (ESP32 + SX1276). It receives
LoRa `MeshEnvelope` packets, logs them with `esp-println` over UART0 (CP2102 USB,
921600 baud), and relays unseen packets. There is exactly **one** USB serial
stream, already shared by `esp-println` logs and `esp-backtrace` panic output.

The goal is to feed live mesh traffic into the host as parseable data. The crate
is being repurposed from *debugger* to *bridge* (renamed `flint-bridge`). The
shared `flint-proto` crate (`no_std`, used on node, bridge, and the std gateway)
already owns the air protocol (`MeshEnvelope`, `FlintPayload`, `decode`/`encode`,
`SeenCache`) and is the natural home for the new host-facing record format.

## Goals / Non-Goals

**Goals:**
- A stable, self-delimiting **binary** record format the host can parse and
  resync, defined once in `flint-proto`.
- The same decode path works whether bytes come from `flint-bridge` over USB or,
  later, from a LoRa radio wired directly into the gateway.
- A clean default wire (binary only) with an on-demand human-readable debug mode
  for debugging the bridge itself, toggled over serial.
- A working host reader in `flint-gateway`.

**Non-Goals:**
- Full Meshtastic interop / protobuf migration (already tracked separately in
  `flint-proto`).
- Bidirectional mesh control / downlink to nodes (the command byte is the seed of
  a host→device channel, but only mode control is in scope here).
- Persisting/forwarding frames beyond pretty-printing in the gateway.
- A reusable standalone `flint-debug` watch tool (may be re-created later).

## Decisions

### Binary records over a text line protocol
Chosen so the gateway can later ingest a raw radio with the *same* decoder — the
wire format becomes the contract and the byte source is interchangeable. Text
lines were considered (trivially coexist with logs, human-debuggable) but they
would force the host to re-parse fields and would not match a future radio path.
Bandwidth is irrelevant either way (SF7, infrequent packets), so the radio-reuse
argument decides it. Human-readability is recovered as a *mode*, not the default.

### Framing: sync word + tag + length + body + CRC
A record is `[sync][type tag][len][body][CRC]`. The sync word lets a parser scan
to the next boundary; `len` bounds the body; the CRC (e.g. CRC-16/CCITT) rejects
corruption. This is necessary because the binary stream is **not** clean — see
the panic-bypass risk below — so the parser must tolerate and skip arbitrary
non-record bytes. COBS framing was considered; sync+len+CRC is simpler to specify
and to decode on the host and is robust enough given infrequent records.

### Record bodies reuse the air protocol
`FrameReport` body = a small header (`rssi`, `snr`, `len`, `disposition`) followed
by the **encoded envelope bytes**, so the host recovers all envelope/sensor
fields via the existing `flint_proto::decode`. The only genuinely new fields are
link quality and disposition (neither lives in the envelope). `RawFrame` body =
the same link-quality header plus the raw received bytes (for packets that do not
decode). Two distinct type tags separate the variants. This avoids duplicating
the sensor schema in a second struct.

### One mode switch drives both format and log noise
Output mode is a single state: `Binary` (default) or `Debug`. The `log` crate's
runtime `log::set_max_level()` is the gate:
- `Binary` → `set_max_level(Off)`: `info!`/`trace!` short-circuit before reaching
  the logger, so no text pollutes the binary stream; emit binary records.
- `Debug` → `set_max_level(Trace)`: full firmware logs return *and* frames are
  rendered as human text instead of binary — for debugging the bridge.

To make trace actually reachable, `ESP_LOG` must be raised to `TRACE` in
`.cargo/config.toml` (it currently caps `esp-println` at `INFO` at compile time);
the runtime `set_max_level` then gates emission Off↔Trace. Alternatives
considered: a GPIO strap pin (needs physical access, burns a pin, boot-only) and
a compile-time feature (needs a reflash to switch) — both rejected in favour of
the serial command, which also seeds a future host→device channel.

### Command channel: single command byte
The bridge runs a UART RX task; `b` → binary, `d` → debug. Defined as explicit
*set-mode* bytes (not a toggle) so they are idempotent and a reconnecting host
can force a known state. Unknown bytes are ignored. Most host monitors
(`espflash --monitor`, `picocom`, `screen`, and the gateway itself) forward
stdin to the port, so this works during bring-up.

### Reader lives in `flint-gateway`
The codec (record types, framing, CRC, `Display`) lives in `flint-proto` so it is
shared and IO-free (`no_std`, no serial deps). The serial-reading binary lives in
`flint-gateway` (already `std`), which is the eventual real consumer — so the
bring-up tool becomes the gateway's first feature rather than throwaway code, and
`flint-proto` stays a pure library. Co-locating the reader in `flint-proto` (a
feature-gated `[[bin]]` with an optional `serialport` dep) was considered and
rejected to keep the shared crate dependency-clean.

## Risks / Trade-offs

- **Panic/boot text bypasses the log gate** → `esp-backtrace` output and early
  boot text are emitted regardless of `set_max_level`, so the binary stream is
  never guaranteed clean. *Mitigation:* this is exactly why framing uses
  sync word + length + CRC; the host skips non-record bytes and resyncs. It is an
  argument *for* the framing, not a defect.
- **Mode switch changes the stream format mid-flight** → a host parser reading
  binary will see human text appear after a `d`. *Mitigation:* debug mode is for a
  human at a terminal, not for the parser; the gateway reader runs the bridge in
  binary mode. The set-mode bytes let the gateway assert binary on connect.
- **Crate rename churn** → the directory/bin/README rename touches paths and any
  references (`Makefile`, root docs). *Mitigation:* it is a standalone workspace;
  grep for `flint-debug` references and update them as part of the rename task.
- **CRC/sync choices must match on both ends** → divergence silently drops every
  record. *Mitigation:* both ends call the same `flint-proto` encode/decode; add a
  host-side round-trip unit test over the codec.

## Open Questions

- Exact sync word and CRC variant (e.g. CRC-16/CCITT-FALSE) — settle in
  implementation; must be identical on both ends and tested.
- Whether `disposition` should also distinguish "relay attempted but TX failed"
  from "relayed", or fold TX errors into logs only.
