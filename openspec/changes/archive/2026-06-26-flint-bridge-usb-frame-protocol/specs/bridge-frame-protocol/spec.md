## ADDED Requirements

### Requirement: Self-delimiting framing with integrity check

The bridge protocol SHALL define a self-delimiting binary record so a host parser
can locate record boundaries, resynchronise after dropped or corrupt bytes, and
reject damaged records. Each record MUST begin with a fixed sync word, carry a
type tag and a body length, and end with a CRC trailer computed over the record
body. The encode and decode of this framing MUST live in `flint-proto` so the
device producer and host consumer share one definition.

#### Scenario: Host resynchronises after stray bytes

- **WHEN** the stream contains non-record bytes before a record (e.g. boot text
  or a panic backtrace) or the parser joins mid-record
- **THEN** the parser can scan forward to the next sync word and decode the next
  complete record

#### Scenario: Corrupt record is rejected

- **WHEN** a record's CRC does not match its body
- **THEN** the parser rejects that record rather than emitting corrupt field
  values

### Requirement: Decoded frame report record

`flint-bridge` SHALL emit exactly one `FrameReport` record, identified by its
type tag, for every received LoRa packet that decodes as a `MeshEnvelope`. The
record MUST carry the link quality (`rssi`, `snr`, byte `len`), the receiver's
disposition of the frame, and the envelope contents in a form the host can decode
with `flint-proto` (e.g. the encoded envelope bytes).

#### Scenario: Sensor reading is received

- **WHEN** a received packet decodes as a `MeshEnvelope` carrying a
  `FlintPayload::SensorReading`
- **THEN** `flint-bridge` emits one `FrameReport` record from which the host can
  recover the link quality, the disposition, and every envelope/sensor field

### Requirement: Frame disposition

Each `FrameReport` SHALL indicate how the receiver disposed of the frame:
relayed onward, delivered locally only, or discarded as a duplicate.

#### Scenario: Frame is relayed

- **WHEN** a decoded frame is unseen and has `hop_limit > 0` and is rebroadcast
- **THEN** its `FrameReport` marks the frame as relayed with the decremented hop
  count that was transmitted

#### Scenario: Frame is local-only

- **WHEN** a decoded frame is unseen and has `hop_limit == 0`
- **THEN** its `FrameReport` marks the frame as locally delivered with no relay

#### Scenario: Frame is a duplicate

- **WHEN** a decoded frame's `packet_id` is already in the seen cache
- **THEN** its `FrameReport` marks the frame as a discarded duplicate and the
  frame is not relayed

### Requirement: Raw frame report record

`flint-bridge` SHALL emit exactly one `RawFrame` record, identified by a type tag
distinct from `FrameReport`, for every received LoRa packet that fails to decode
as a `MeshEnvelope`, carrying the link quality and the raw received bytes, so the
host observes every reception.

#### Scenario: Undecodable packet is received

- **WHEN** a received packet does not decode as a `MeshEnvelope`
- **THEN** `flint-bridge` emits one `RawFrame` record with the link quality and
  the raw bytes, distinguishable by tag from a `FrameReport`

### Requirement: Host decoder in the gateway

`flint-gateway` SHALL read the bridge stream from the serial port, decode records
using the `flint-proto` definitions, and present them. It MUST distinguish record
types and skip non-record bytes without aborting.

#### Scenario: Gateway decodes a live stream

- **WHEN** the gateway is connected to a `flint-bridge` emitting binary records
- **THEN** it decodes each `FrameReport` and `RawFrame`, renders it, and
  continues past any stray bytes between records
