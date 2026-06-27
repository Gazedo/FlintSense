//! flint-proto — shared packet types and mesh routing primitives.
//!
//! This crate is `no_std` and is shared between:
//!   - embedded sensor nodes  (flint-node)
//!   - the bridge/relay node  (flint-bridge)
//!   - the RPi gateway        (flint-gateway, std)
//!
//! ## Mesh routing model
//!
//! Flood routing with hop-limit deduplication — identical semantics to Meshtastic:
//!
//!   1. Sender wraps payload in `MeshEnvelope` with a random `packet_id` and
//!      `hop_limit = DEFAULT_HOP_LIMIT`.
//!   2. Any receiving node checks its `SeenCache`:
//!      - **Duplicate** → discard silently.
//!      - **Unseen, hop_limit == 0** → deliver locally, do not rebroadcast.
//!      - **Unseen, hop_limit > 0** → deliver locally, decrement `hop_limit`,
//!        rebroadcast, add `packet_id` to cache.
//!
//! ## Path to full Meshtastic interop (Option B)
//!
//! To join an actual Meshtastic network you would replace `MeshEnvelope` with a
//! protobuf-encoded `MeshPacket` (use the `micropb` crate for no_std protobuf),
//! match the Meshtastic channel modem config exactly (US 915 MHz LongFast =
//! SF11 / BW250 / CR4/8 / sync word 0x2B), and apply AES-256 channel encryption
//! using the channel PSK.  This crate is structured so that change is isolated to
//! the envelope layer — `WeatherPacket` and `FlintPayload` would remain the same.

#![cfg_attr(not(test), no_std)]

use heapless::Vec;
use serde::{Deserialize, Serialize};

// ── Constants ────────────────────────────────────────────────────────────────

/// Flood-broadcast destination (deliver to every node in range).
pub const BROADCAST_ADDR: u32 = 0xFFFF_FFFF;

/// Default hop limit — matches Meshtastic's default of 3.
/// A value of 3 reaches ~4 hops from the origin before packets are dropped.
pub const DEFAULT_HOP_LIMIT: u8 = 3;

/// Capacity of the seen-packet deduplication cache (number of packet IDs).
pub const SEEN_CACHE_SIZE: usize = 64;

// ── Power status flags ────────────────────────────────────────────────────────

/// Bit flags carried in [`WeatherPacket::power_flags`].
pub mod power_flags {
    /// SOC has fallen below the low-battery threshold (default 20 %).
    pub const LOW_SOC: u8 = 1 << 0;
    /// Battery is currently discharging (CRATE < 0) — no net solar harvest.
    pub const NEGATIVE_TREND: u8 = 1 << 1;
    /// Solar charger is actively delivering current to the battery.
    pub const CHARGING: u8 = 1 << 2;
    /// SOC has fallen below the critical threshold (default 10 %) —
    /// gateway should alert immediately.
    pub const CRITICAL_SOC: u8 = 1 << 3;
}

// ── Payload types ────────────────────────────────────────────────────────────

/// Fire-weather sensor reading from a single node.
///
/// All values use scaled integers to avoid floating point on embedded targets.
///
/// ## Wire size
/// Postcard-encoded inside a `MeshEnvelope`: ~33 bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct WeatherPacket {
    /// Which node sent this reading (0–254; 255 reserved).
    pub node_id: u8,

    /// Air temperature in Celsius × 100  (e.g. 2350 → 23.50 °C).
    pub temp_c: i16,

    /// Relative humidity, 0–100 %.
    pub humidity_pct: u8,

    /// BME680 MOX gas-sensor resistance in ohms — a total-VOC proxy. Higher =
    /// cleaner air; a sharp drop indicates VOCs / combustion smoke. Raw value:
    /// IAQ baseline/index math is done downstream on the gateway. 0 = no valid
    /// reading (sensor warming up, or gas measurement disabled).
    pub gas_resistance_ohms: u32,

    /// Wind speed in m/s × 2  (e.g. 14 → 7.0 m/s, max ~127 m/s).
    pub wind_speed_ms: u8,

    /// Wind direction, 0–359 °.
    pub wind_dir_deg: u16,

    /// Dead fuel moisture proxy, 0–100 %.
    /// Derived from resistance measurement across a wood dowel.
    pub fuel_moisture: u8,

    /// Battery state-of-charge, 0–100 % (from MAX17048 fuel gauge).
    pub battery_soc: u8,

    /// Battery terminal voltage in millivolts.
    pub battery_mv: u16,

    /// Instantaneous solar charge current in milliamps (from INA219 on charge line).
    /// Zero when no solar input.
    pub solar_ma: u16,

    /// Instantaneous node load current in milliamps (from INA219 on 3.3 V rail).
    pub load_ma: u16,

    /// Power status bit flags — see [`power_flags`] constants.
    pub power_flags: u8,

    /// Monotonic per-node packet counter — use to detect dropped packets.
    pub sequence: u16,
}

/// All application-layer message types the network can carry.
///
/// Keeping this as an enum means future message types (node advertisements,
/// ACKs, gateway config pushes) don't require a protocol version bump.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum FlintPayload {
    SensorReading(WeatherPacket),
    // Future variants:
    // NodeInfo(NodeInfoPacket),
    // GatewayAck(u16),          // echoes sequence number
}

// ── Mesh envelope ────────────────────────────────────────────────────────────

/// Mesh routing envelope — wraps any `FlintPayload` for flood routing.
///
/// Serialized with postcard for wire-efficiency (~4 bytes overhead over payload).
/// See module-level docs for routing behaviour and Meshtastic interop notes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct MeshEnvelope {
    /// Originating node ID.  Recommended: lower 4 bytes of the chip's MAC address.
    pub from: u32,

    /// Destination node ID, or `BROADCAST_ADDR` for network-wide delivery.
    pub to: u32,

    /// Randomly-generated unique packet ID — the key for deduplication.
    pub packet_id: u32,

    /// Remaining relay hops allowed before the packet is silently dropped.
    pub hop_limit: u8,

    /// Original `hop_limit` at time of first transmission.
    /// Retained so gateways can compute path quality (hops consumed = hop_start − hop_limit).
    pub hop_start: u8,

    pub payload: FlintPayload,
}

impl MeshEnvelope {
    /// Convenience constructor for a new broadcast packet.
    pub fn new_broadcast(from: u32, packet_id: u32, payload: FlintPayload) -> Self {
        Self {
            from,
            to: BROADCAST_ADDR,
            packet_id,
            hop_limit: DEFAULT_HOP_LIMIT,
            hop_start: DEFAULT_HOP_LIMIT,
            payload,
        }
    }

    /// Returns a copy of this envelope prepared for rebroadcast (hop_limit decremented).
    /// Returns `None` if hop_limit is already 0 and the packet should not be relayed.
    pub fn for_relay(&self) -> Option<Self> {
        if self.hop_limit == 0 {
            return None;
        }
        let mut relayed = self.clone();
        relayed.hop_limit -= 1;
        Some(relayed)
    }
}

// ── Seen-packet deduplication cache ─────────────────────────────────────────

/// Fixed-capacity ring cache of recently-seen packet IDs.
///
/// Used by every node to suppress duplicate rebroadcasts in the flood mesh.
/// When the cache is full the oldest half is evicted (simple sliding window).
pub struct SeenCache {
    packets: Vec<u32, SEEN_CACHE_SIZE>,
    /// Index of the next write position (ring-buffer eviction).
    write_pos: usize,
}

impl SeenCache {
    pub const fn new() -> Self {
        Self {
            packets: Vec::new(),
            write_pos: 0,
        }
    }

    /// Returns `true` if `packet_id` is **new** (and records it).
    /// Returns `false` if it is a duplicate — the caller should discard the packet.
    pub fn check_and_insert(&mut self, packet_id: u32) -> bool {
        if self.packets.contains(&packet_id) {
            return false;
        }

        if self.packets.is_full() {
            // Overwrite at write_pos (ring-buffer eviction of oldest entry).
            self.packets[self.write_pos] = packet_id;
            self.write_pos = (self.write_pos + 1) % SEEN_CACHE_SIZE;
        } else {
            // Still filling up — push normally.
            let _ = self.packets.push(packet_id);
        }

        true
    }

    /// Returns the number of packet IDs currently tracked.
    pub fn len(&self) -> usize {
        self.packets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }
}

impl Default for SeenCache {
    fn default() -> Self {
        Self::new()
    }
}

// ── Wire encoding helpers ────────────────────────────────────────────────────

/// Maximum serialized size of a `MeshEnvelope` containing a `WeatherPacket`.
/// Used to size stack buffers for postcard encoding — update if fields grow.
///
/// Breakdown (postcard, little-endian fixed-width integers):
///   MeshEnvelope header  15 bytes  (from/to/packet_id: 3×u32, hop_limit/hop_start: 2×u8, enum tag: 1)
///   WeatherPacket        ~23 bytes (incl. gas_resistance_ohms: u32, varint ≤5)
///   ─────────────────────────────
///   Total                ~38 bytes → buffer sized to 48 for headroom
pub const MAX_PACKET_BYTES: usize = 48;

/// Serialize a `MeshEnvelope` into a stack-allocated buffer.
///
/// Returns the filled slice on success, or a postcard error.
pub fn encode<'a>(envelope: &MeshEnvelope, buf: &'a mut [u8; MAX_PACKET_BYTES]) -> postcard::Result<&'a [u8]> {
    postcard::to_slice(envelope, buf).map(|s| s as &[u8])
}

/// Deserialize a `MeshEnvelope` from a received byte slice.
pub fn decode(bytes: &[u8]) -> postcard::Result<MeshEnvelope> {
    postcard::from_bytes(bytes)
}

// ── Bridge protocol (flint-bridge → host) ─────────────────────────────────────
//
// `flint-bridge` reports every reception to the host over USB as a self-delimiting
// binary record so a host parser can locate boundaries, resynchronise after stray
// boot/panic text, and reject corruption. The same decode path serves a LoRa radio
// wired directly into the gateway. Both producer (device) and consumer (host) use
// the definitions in this module, so they cannot drift.
//
// ## Frame layout
//
//   [sync:2] [tag:1] [len:1] [body:len] [crc16:2]
//
//   - sync  = `BRIDGE_SYNC` — scanned for to find record starts.
//   - tag   = record type (`BRIDGE_TAG_*`).
//   - len   = body length in bytes.
//   - crc16 = CRC-16/CCITT-FALSE over `tag .. body` (little-endian on the wire).
//
// ## Bodies
//
//   FrameReport: [rssi:i16] [snr:i16] [len:u8] [disposition:u8] [hops_left:u8]
//                [envelope bytes…]   (the received MeshEnvelope, host-decodable)
//   RawFrame:    [rssi:i16] [snr:i16] [len:u8] [raw bytes…]

/// Sync word marking the start of a bridge record ("FlintSEnse").
pub const BRIDGE_SYNC: [u8; 2] = [0xF1, 0x5E];

/// Record type tag: a decoded `MeshEnvelope` report.
pub const BRIDGE_TAG_FRAME_REPORT: u8 = 0x01;

/// Record type tag: an undecodable raw reception.
pub const BRIDGE_TAG_RAW_FRAME: u8 = 0x02;

/// FrameReport body header: rssi(2) + snr(2) + len(1) + disposition(1) + hops_left(1).
const FRAME_HEADER_LEN: usize = 7;

/// RawFrame body header: rssi(2) + snr(2) + len(1).
const RAW_HEADER_LEN: usize = 5;

/// Largest possible record body (FrameReport carrying a full-size envelope).
pub const BRIDGE_MAX_BODY: usize = FRAME_HEADER_LEN + MAX_PACKET_BYTES;

/// Largest possible encoded record: sync(2) + tag(1) + len(1) + body + crc(2).
pub const BRIDGE_MAX_FRAME: usize = 4 + BRIDGE_MAX_BODY + 2;

/// How the receiver disposed of a decoded frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Disposition {
    /// Unseen with `hop_limit > 0` — rebroadcast onward.
    Relayed,
    /// Unseen with `hop_limit == 0` — delivered locally, not relayed.
    LocalOnly,
    /// `packet_id` already in the seen cache — discarded.
    Duplicate,
}

impl Disposition {
    fn to_u8(self) -> u8 {
        match self {
            Disposition::Relayed => 0,
            Disposition::LocalOnly => 1,
            Disposition::Duplicate => 2,
        }
    }

    fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Disposition::Relayed),
            1 => Some(Disposition::LocalOnly),
            2 => Some(Disposition::Duplicate),
            _ => None,
        }
    }
}

impl core::fmt::Display for Disposition {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Disposition::Relayed => "relayed",
            Disposition::LocalOnly => "local",
            Disposition::Duplicate => "duplicate",
        })
    }
}

/// Link-quality header carried by every bridge record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct LinkQuality {
    /// Received signal strength, dBm.
    pub rssi: i16,
    /// Signal-to-noise ratio, dB.
    pub snr: i16,
    /// Number of bytes received off the air.
    pub len: u8,
}

/// Errors that can occur while encoding a bridge record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeError {
    /// The output buffer is smaller than the encoded record.
    BufferTooSmall,
    /// The payload exceeds the protocol maximum.
    BodyTooLong,
}

/// CRC-16/CCITT-FALSE (poly 0x1021, init 0xFFFF, no reflection, xorout 0x0000).
fn crc16_ccitt(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
            bit += 1;
        }
    }
    crc
}

/// Frame a record body with sync word, tag, length, and CRC into `out`.
/// Returns the number of bytes written.
fn write_frame(tag: u8, body: &[u8], out: &mut [u8]) -> Result<usize, BridgeError> {
    if body.len() > u8::MAX as usize {
        return Err(BridgeError::BodyTooLong);
    }
    let total = 4 + body.len() + 2;
    if out.len() < total {
        return Err(BridgeError::BufferTooSmall);
    }
    out[0] = BRIDGE_SYNC[0];
    out[1] = BRIDGE_SYNC[1];
    out[2] = tag;
    out[3] = body.len() as u8;
    out[4..4 + body.len()].copy_from_slice(body);
    let crc = crc16_ccitt(&out[2..4 + body.len()]);
    out[4 + body.len()] = (crc & 0xFF) as u8;
    out[4 + body.len() + 1] = (crc >> 8) as u8;
    Ok(total)
}

/// Encode a `FrameReport` record (decoded `MeshEnvelope` reception) into `out`.
///
/// `envelope_bytes` is the received envelope as encoded by [`encode`]; the host
/// recovers all fields by calling [`decode`] on it. Returns the record length.
pub fn encode_frame_report(
    link: &LinkQuality,
    disposition: Disposition,
    hops_left: u8,
    envelope_bytes: &[u8],
    out: &mut [u8],
) -> Result<usize, BridgeError> {
    if envelope_bytes.len() > MAX_PACKET_BYTES {
        return Err(BridgeError::BodyTooLong);
    }
    let mut body = [0u8; BRIDGE_MAX_BODY];
    body[0..2].copy_from_slice(&link.rssi.to_le_bytes());
    body[2..4].copy_from_slice(&link.snr.to_le_bytes());
    body[4] = link.len;
    body[5] = disposition.to_u8();
    body[6] = hops_left;
    body[FRAME_HEADER_LEN..FRAME_HEADER_LEN + envelope_bytes.len()]
        .copy_from_slice(envelope_bytes);
    let body_len = FRAME_HEADER_LEN + envelope_bytes.len();
    write_frame(BRIDGE_TAG_FRAME_REPORT, &body[..body_len], out)
}

/// Encode a `RawFrame` record (undecodable reception) into `out`.
/// Returns the record length.
pub fn encode_raw_frame(
    link: &LinkQuality,
    raw: &[u8],
    out: &mut [u8],
) -> Result<usize, BridgeError> {
    if raw.len() > MAX_PACKET_BYTES {
        return Err(BridgeError::BodyTooLong);
    }
    let mut body = [0u8; BRIDGE_MAX_BODY];
    body[0..2].copy_from_slice(&link.rssi.to_le_bytes());
    body[2..4].copy_from_slice(&link.snr.to_le_bytes());
    body[4] = link.len;
    body[RAW_HEADER_LEN..RAW_HEADER_LEN + raw.len()].copy_from_slice(raw);
    let body_len = RAW_HEADER_LEN + raw.len();
    write_frame(BRIDGE_TAG_RAW_FRAME, &body[..body_len], out)
}

/// A decoded bridge record.
#[derive(Debug, Clone)]
pub enum BridgeRecord {
    /// A decoded `MeshEnvelope` reception. `envelope` holds the received envelope
    /// bytes — call [`decode`] on it to recover the typed packet.
    Frame {
        link: LinkQuality,
        disposition: Disposition,
        hops_left: u8,
        envelope: Vec<u8, MAX_PACKET_BYTES>,
    },
    /// An undecodable reception, with the raw bytes received off the air.
    Raw {
        link: LinkQuality,
        data: Vec<u8, MAX_PACKET_BYTES>,
    },
}

impl BridgeRecord {
    /// Build a `Frame` record from received envelope bytes (truncated to capacity).
    /// Lets producers render via [`Display`](core::fmt::Display) without depending
    /// on `heapless` directly.
    pub fn frame(
        link: LinkQuality,
        disposition: Disposition,
        hops_left: u8,
        envelope_bytes: &[u8],
    ) -> Self {
        let mut envelope = Vec::new();
        let _ = envelope.extend_from_slice(&envelope_bytes[..envelope_bytes.len().min(MAX_PACKET_BYTES)]);
        BridgeRecord::Frame {
            link,
            disposition,
            hops_left,
            envelope,
        }
    }

    /// Build a `Raw` record from received bytes (truncated to capacity).
    pub fn raw(link: LinkQuality, data_bytes: &[u8]) -> Self {
        let mut data = Vec::new();
        let _ = data.extend_from_slice(&data_bytes[..data_bytes.len().min(MAX_PACKET_BYTES)]);
        BridgeRecord::Raw { link, data }
    }
}

/// Result of attempting to parse one record from a streaming buffer.
#[derive(Debug)]
pub enum ParseOutcome {
    /// A complete, CRC-valid record. Drop `consumed` leading bytes and continue.
    Record { record: BridgeRecord, consumed: usize },
    /// Not enough bytes buffered yet to decide — read more and retry.
    NeedMore,
    /// Discard `skip` leading bytes (no sync, bad CRC, or unknown tag) and retry.
    Skip(usize),
}

/// Locate the first sync word in `buf`.
fn find_sync(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == BRIDGE_SYNC)
}

fn parse_body(tag: u8, body: &[u8]) -> Option<BridgeRecord> {
    match tag {
        BRIDGE_TAG_FRAME_REPORT => {
            if body.len() < FRAME_HEADER_LEN {
                return None;
            }
            let link = LinkQuality {
                rssi: i16::from_le_bytes([body[0], body[1]]),
                snr: i16::from_le_bytes([body[2], body[3]]),
                len: body[4],
            };
            let disposition = Disposition::from_u8(body[5])?;
            let hops_left = body[6];
            let mut envelope = Vec::new();
            envelope.extend_from_slice(&body[FRAME_HEADER_LEN..]).ok()?;
            Some(BridgeRecord::Frame {
                link,
                disposition,
                hops_left,
                envelope,
            })
        }
        BRIDGE_TAG_RAW_FRAME => {
            if body.len() < RAW_HEADER_LEN {
                return None;
            }
            let link = LinkQuality {
                rssi: i16::from_le_bytes([body[0], body[1]]),
                snr: i16::from_le_bytes([body[2], body[3]]),
                len: body[4],
            };
            let mut data = Vec::new();
            data.extend_from_slice(&body[RAW_HEADER_LEN..]).ok()?;
            Some(BridgeRecord::Raw { link, data })
        }
        _ => None,
    }
}

/// Attempt to parse a single bridge record from the front of `buf`.
///
/// Drives a streaming host parser: feed serial bytes into a growing buffer, call
/// this repeatedly, and act on the [`ParseOutcome`] (drop `consumed`/`skip` bytes,
/// or read more on `NeedMore`). Stray bytes between records — boot text, panic
/// backtraces — are skipped until the next valid, CRC-checked record.
pub fn parse_record(buf: &[u8]) -> ParseOutcome {
    match find_sync(buf) {
        // No sync yet. Keep the last byte in case it is a split sync word.
        None => {
            let skip = buf.len().saturating_sub(1);
            if skip == 0 {
                ParseOutcome::NeedMore
            } else {
                ParseOutcome::Skip(skip)
            }
        }
        // Align the buffer so the sync word is at offset 0.
        Some(pos) if pos > 0 => ParseOutcome::Skip(pos),
        Some(_) => {
            if buf.len() < 4 {
                return ParseOutcome::NeedMore;
            }
            let tag = buf[2];
            let len = buf[3] as usize;
            let total = 4 + len + 2;
            if buf.len() < total {
                return ParseOutcome::NeedMore;
            }
            let crc_calc = crc16_ccitt(&buf[2..4 + len]);
            let crc_wire = (buf[4 + len] as u16) | ((buf[5 + len] as u16) << 8);
            if crc_calc != crc_wire {
                // Corrupt: step past this sync byte and look for the next one.
                return ParseOutcome::Skip(2);
            }
            match parse_body(tag, &buf[4..4 + len]) {
                Some(record) => ParseOutcome::Record {
                    record,
                    consumed: total,
                },
                None => ParseOutcome::Skip(2),
            }
        }
    }
}

impl core::fmt::Display for BridgeRecord {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BridgeRecord::Frame {
                link,
                disposition,
                hops_left,
                envelope,
            } => {
                write!(
                    f,
                    "FRAME rssi={}dBm snr={}dB len={} {} hops_left={}",
                    link.rssi, link.snr, link.len, disposition, hops_left
                )?;
                match decode(envelope) {
                    Ok(env) => {
                        write!(
                            f,
                            " from=0x{:08x} id=0x{:08x} hops={}/{}",
                            env.from, env.packet_id, env.hop_limit, env.hop_start
                        )?;
                        match &env.payload {
                            FlintPayload::SensorReading(w) => write!(
                                f,
                                " node={} {}.{:02}C {}%RH gas={}ohm \
                                 {}x0.5m/s@{}deg fuel={}% batt={}%/{}mV seq={}",
                                w.node_id,
                                w.temp_c / 100,
                                w.temp_c.unsigned_abs() % 100,
                                w.humidity_pct,
                                w.gas_resistance_ohms,
                                w.wind_speed_ms,
                                w.wind_dir_deg,
                                w.fuel_moisture,
                                w.battery_soc,
                                w.battery_mv,
                                w.sequence,
                            ),
                        }
                    }
                    Err(_) => write!(f, " <undecodable envelope>"),
                }
            }
            BridgeRecord::Raw { link, data } => {
                write!(
                    f,
                    "RAW rssi={}dBm snr={}dB len={} bytes=",
                    link.rssi, link.snr, link.len
                )?;
                for b in data {
                    write!(f, "{:02x}", b)?;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod bridge_tests {
    use super::*;

    fn sample_envelope() -> MeshEnvelope {
        MeshEnvelope::new_broadcast(
            0x0a2b_3c4d,
            0xdead_beef,
            FlintPayload::SensorReading(WeatherPacket {
                node_id: 7,
                temp_c: 2350,
                humidity_pct: 42,
                gas_resistance_ohms: 51234,
                wind_speed_ms: 14,
                wind_dir_deg: 270,
                fuel_moisture: 12,
                battery_soc: 88,
                battery_mv: 4012,
                solar_ma: 120,
                load_ma: 35,
                power_flags: 0,
                sequence: 9,
            }),
        )
    }

    #[test]
    fn frame_report_round_trip() {
        let env = sample_envelope();
        let mut envbuf = [0u8; MAX_PACKET_BYTES];
        let env_bytes = encode(&env, &mut envbuf).unwrap();
        let link = LinkQuality {
            rssi: -91,
            snr: 7,
            len: env_bytes.len() as u8,
        };

        let mut out = [0u8; BRIDGE_MAX_FRAME];
        let n =
            encode_frame_report(&link, Disposition::Relayed, 2, env_bytes, &mut out).unwrap();

        match parse_record(&out[..n]) {
            ParseOutcome::Record { record, consumed } => {
                assert_eq!(consumed, n);
                match record {
                    BridgeRecord::Frame {
                        link: l,
                        disposition,
                        hops_left,
                        envelope,
                    } => {
                        assert_eq!(l, link);
                        assert_eq!(disposition, Disposition::Relayed);
                        assert_eq!(hops_left, 2);
                        let decoded = decode(&envelope).unwrap();
                        assert_eq!(decoded.from, env.from);
                        assert_eq!(decoded.packet_id, env.packet_id);
                    }
                    other => panic!("expected Frame, got {other:?}"),
                }
            }
            other => panic!("expected Record, got {other:?}"),
        }
    }

    #[test]
    fn raw_frame_round_trip() {
        let raw = [0x01, 0x02, 0x03, 0xfe, 0xff];
        let link = LinkQuality {
            rssi: -120,
            snr: -8,
            len: raw.len() as u8,
        };
        let mut out = [0u8; BRIDGE_MAX_FRAME];
        let n = encode_raw_frame(&link, &raw, &mut out).unwrap();

        match parse_record(&out[..n]) {
            ParseOutcome::Record { record, consumed } => {
                assert_eq!(consumed, n);
                match record {
                    BridgeRecord::Raw { link: l, data } => {
                        assert_eq!(l, link);
                        assert_eq!(&data[..], &raw[..]);
                    }
                    other => panic!("expected Raw, got {other:?}"),
                }
            }
            other => panic!("expected Record, got {other:?}"),
        }
    }

    #[test]
    fn resyncs_after_leading_garbage() {
        let link = LinkQuality {
            rssi: -70,
            snr: 9,
            len: 3,
        };
        let mut frame = [0u8; BRIDGE_MAX_FRAME];
        let n = encode_raw_frame(&link, &[0xaa, 0xbb, 0xcc], &mut frame).unwrap();

        // Prepend boot/panic-like garbage (including a lone sync byte) before the frame.
        let mut stream = std::vec::Vec::new();
        stream.extend_from_slice(b"booting...\xf1 panic\r\n");
        stream.extend_from_slice(&frame[..n]);

        // Drive the parser the way the gateway would, skipping until the record.
        let mut pos = 0;
        loop {
            match parse_record(&stream[pos..]) {
                ParseOutcome::Record { record, consumed } => {
                    match record {
                        BridgeRecord::Raw { data, .. } => {
                            assert_eq!(&data[..], &[0xaa, 0xbb, 0xcc]);
                        }
                        other => panic!("expected Raw, got {other:?}"),
                    }
                    assert_eq!(consumed, n);
                    break;
                }
                ParseOutcome::Skip(k) => {
                    assert!(k > 0);
                    pos += k;
                }
                ParseOutcome::NeedMore => panic!("ran out of bytes before finding record"),
            }
        }
    }

    #[test]
    fn rejects_corrupt_crc() {
        let link = LinkQuality {
            rssi: -70,
            snr: 9,
            len: 3,
        };
        let mut frame = [0u8; BRIDGE_MAX_FRAME];
        let n = encode_raw_frame(&link, &[0xaa, 0xbb, 0xcc], &mut frame).unwrap();
        // Flip a body byte so the CRC no longer matches.
        frame[5] ^= 0xff;

        match parse_record(&frame[..n]) {
            ParseOutcome::Skip(k) => assert_eq!(k, 2),
            other => panic!("expected Skip on bad CRC, got {other:?}"),
        }
    }
}
