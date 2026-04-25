//! flint-proto — shared packet types and mesh routing primitives.
//!
//! This crate is `no_std` and is shared between:
//!   - embedded sensor nodes  (flint-node)
//!   - the debug/relay node   (flint-debug)
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

#![no_std]

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
///   WeatherPacket        18 bytes  (see struct fields)
///   ─────────────────────────────
///   Total                33 bytes  → buffer sized to 48 for headroom
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
