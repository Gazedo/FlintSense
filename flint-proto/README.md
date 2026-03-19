# flint-proto

Shared packet types and mesh routing primitives for the FlintSense sensor
network.

This crate is `no_std` and is a dependency of every other crate in the
workspace — embedded sensor nodes, the debug/relay node, and the Raspberry Pi
gateway all use the same types to guarantee wire-format compatibility.

## Contents

| Item | Description |
|---|---|
| `WeatherPacket` | Fire-weather sensor reading (temperature, humidity, wind, fuel moisture, battery) |
| `FlintPayload` | Enum of all application-layer message types |
| `MeshEnvelope` | Flood-routing wrapper — same hop-limit semantics as Meshtastic |
| `SeenCache` | Fixed-capacity ring cache for seen-packet deduplication |
| `encode` / `decode` | postcard serialization helpers |

## Feature Flags

| Feature | Description |
|---|---|
| `defmt` | Derives `defmt::Format` on all public types for RTT logging on embedded targets |

## Generating Docs

```bash
cargo doc -p flint-proto --open
```
