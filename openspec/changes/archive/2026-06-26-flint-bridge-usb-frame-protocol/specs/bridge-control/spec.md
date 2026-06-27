## ADDED Requirements

### Requirement: Host-to-device command channel

`flint-bridge` SHALL listen on the USB serial port for single-byte commands from
the host while continuing to receive LoRa frames. The byte `b` selects binary
output mode and the byte `d` selects debug output mode. Commands MUST be
idempotent — sending the byte for the current mode leaves the bridge in that
mode — so a reconnecting host can force a known state. Unrecognised bytes MUST be
ignored without disrupting frame reception.

#### Scenario: Host selects debug mode

- **WHEN** the host sends `d` over serial
- **THEN** the bridge enters debug mode

#### Scenario: Host forces binary mode

- **WHEN** the host sends `b` while already in binary mode
- **THEN** the bridge remains in binary mode and continues emitting binary records

#### Scenario: Unknown command is ignored

- **WHEN** the host sends a byte other than `b` or `d`
- **THEN** the bridge ignores it and continues receiving and reporting frames

### Requirement: Binary mode is the default and silences logs

`flint-bridge` SHALL start in binary mode. In binary mode it MUST set the runtime
log level so that `info!`/`trace!` output is suppressed and does not interleave
with the binary record stream, and it MUST emit `FrameReport`/`RawFrame` binary
records for received frames.

#### Scenario: Logs do not pollute the binary stream

- **WHEN** the bridge is in binary mode (its default at boot) and a frame is
  received
- **THEN** no human-readable log line is written to serial and only the binary
  record(s) for that frame are emitted

### Requirement: Debug mode emits human-readable output with trace logging

In debug mode `flint-bridge` SHALL raise the runtime log level to trace and emit
human-readable frame text *in place of* the binary record stream, so the bridge
itself can be debugged. Switching back to binary mode MUST restore log
suppression and binary records.

#### Scenario: Debug mode shows frames and trace logs

- **WHEN** the bridge is in debug mode and a frame is received
- **THEN** it emits human-readable text for that frame and firmware log output up
  to trace level is enabled, and no binary records are emitted

#### Scenario: Returning to binary mode restores the clean stream

- **WHEN** the bridge is in debug mode and the host sends `b`
- **THEN** the bridge suppresses logs again and resumes emitting binary records
