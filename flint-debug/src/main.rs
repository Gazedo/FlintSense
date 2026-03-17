//! flint-debug — LoRa receive + mesh relay firmware for Heltec WiFi LoRa 32 V2.
//!
//! ## Bring-up checklist
//!
//!   1. Flash the Heltec Arduino LoRa TX example on a second V2 board.
//!   2. Note the exact SF / BW / CR / frequency from that sketch and set the
//!      constants below to match.  Arduino-LoRa defaults are shown inline.
//!   3. Flash this firmware: `cargo run --release` (from this crate directory).
//!   4. Any received packet that doesn't decode as a MeshEnvelope is hex-dumped
//!      so you can inspect raw Arduino packets before full Rust TX is ready.
//!
//! ## Heltec V2 pin mapping (SX1276 via SPI2)
//!
//!   SCK GPIO5  MISO GPIO19  MOSI GPIO27  CS GPIO18  RST GPIO14  DIO0 GPIO26
//!
//! ## Meshtastic US LongFast (future interop target)
//!
//!   freq=906_875_000  SF11  BW250  CR4/8  use_public_network=true

#![no_std]
#![no_main]

use defmt::{error, info, warn};
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_backtrace as _;
use esp_hal::{
    gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull},
    spi::{
        master::{Config as SpiConfig, Spi},
        Mode,
    },
    time::Rate,
    timer::timg::TimerGroup,
};
use flint_proto::{decode, encode, FlintPayload, SeenCache, MAX_PACKET_BYTES};
use lora_phy::{
    iv::GenericSx127xInterfaceVariant,
    mod_params::{Bandwidth, CodingRate, SpreadingFactor},
    sx127x::{Config as Sx127xConfig, Sx127x, Sx1276},
    LoRa, RxMode,
};

esp_bootloader_esp_idf::esp_app_desc!();

// defmt 1.0 requires a timestamp implementation.
// Using embassy_time for microsecond-resolution timestamps.
defmt::timestamp!("{=u64:us}", embassy_time::Instant::now().as_micros());

// ── LoRa channel config ───────────────────────────────────────────────────────
//
// Match these to your Arduino TX sketch.
// Arduino-LoRa library defaults (shown for reference):
//   LoRa.begin(915E6)              → LORA_FREQUENCY_HZ = 915_000_000
//   LoRa.setSpreadingFactor(7)     → SpreadingFactor::_7
//   LoRa.setSignalBandwidth(125E3) → Bandwidth::_125KHz
//   LoRa.setCodingRate4(5)         → CodingRate::_4_5
//   LoRa.setSyncWord(0x12)         → use_public_network = false

const LORA_FREQUENCY_HZ: u32 = 915_000_000;
const LORA_SF: SpreadingFactor = SpreadingFactor::_7;
const LORA_BW: Bandwidth = Bandwidth::_125KHz;
const LORA_CR: CodingRate = CodingRate::_4_5;
const LORA_PUBLIC_NETWORK: bool = false;
const LORA_TX_POWER_DBM: i32 = 14;

// ── Entry point ───────────────────────────────────────────────────────────────

#[esp_rtos::main]
async fn main(_spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default());

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    info!("kindle-debug booting");

    // ── SPI2 for SX1276 ──────────────────────────────────────────────────────

    let cs = Output::new(peripherals.GPIO18, Level::High, OutputConfig::default());

    let spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(1))
            .with_mode(Mode::_0),
    )
    .unwrap()
    .with_sck(peripherals.GPIO5)
    .with_miso(peripherals.GPIO19)
    .with_mosi(peripherals.GPIO27)
    .into_async();

    // ExclusiveDevice wraps SpiBus + CS pin into the SpiDevice trait lora-phy needs.
    let spi_dev = ExclusiveDevice::new(spi, cs, Delay).unwrap();

    // ── SX1276 interface variant (reset + DIO0 IRQ + optional RF switches) ────

    let reset = Output::new(peripherals.GPIO14, Level::High, OutputConfig::default());
    let dio0 = Input::new(peripherals.GPIO26, InputConfig::default().with_pull(Pull::Down));

    let iv = GenericSx127xInterfaceVariant::new(
        reset,
        dio0,
        None, // rf_switch_rx — not needed on Heltec V2
        None, // rf_switch_tx — not needed on Heltec V2
    )
    .unwrap();

    // ── Radio init ────────────────────────────────────────────────────────────

    let sx_config = Sx127xConfig {
        chip: Sx1276,
        tcxo_used: false,
        tx_boost: false, // PA_BOOST pin — false uses RFO for lower power (<14 dBm)
        rx_boost: false,
    };

    let mut lora = LoRa::new(Sx127x::new(spi_dev, iv, sx_config), LORA_PUBLIC_NETWORK, Delay)
        .await
        .expect("LoRa init failed — check SPI wiring and pin assignments");

    // ── Modulation + packet params ────────────────────────────────────────────

    let mdm = lora
        .create_modulation_params(LORA_SF, LORA_BW, LORA_CR, LORA_FREQUENCY_HZ)
        .expect("invalid modulation params");

    let rx_params = lora
        .create_rx_packet_params(8, false, MAX_PACKET_BYTES as u8, true, false, &mdm)
        .expect("invalid rx packet params");

    let mut tx_params = lora
        .create_tx_packet_params(8, false, true, false, &mdm)
        .expect("invalid tx packet params");

    let mut seen = SeenCache::new();

    info!(
        "RX loop — {}Hz  SF{}  BW{:?}  CR{:?}",
        LORA_FREQUENCY_HZ, LORA_SF as u8, LORA_BW, LORA_CR,
    );

    // ── Main receive loop ─────────────────────────────────────────────────────

    loop {
        if let Err(e) = lora.prepare_for_rx(RxMode::Single(0), &mdm, &rx_params).await {
            error!("prepare_for_rx: {:?}", defmt::Debug2Format(&e));
            continue;
        }

        let mut rx_buf = [0u8; MAX_PACKET_BYTES];

        let (len, status) = match lora.rx(&rx_params, &mut rx_buf).await {
            Ok(r) => r,
            Err(e) => {
                warn!("rx: {:?}", defmt::Debug2Format(&e));
                continue;
            }
        };

        let raw = &rx_buf[..len as usize];
        info!("RX {} bytes  RSSI={}dBm  SNR={}dB", len, status.rssi, status.snr);

        // ── Decode ────────────────────────────────────────────────────────────

        match decode(raw) {
            Ok(envelope) => {
                info!(
                    "  KindlePacket  from=0x{:08x}  id=0x{:08x}  hops={}/{}",
                    envelope.from, envelope.packet_id, envelope.hop_limit, envelope.hop_start,
                );

                match &envelope.payload {
                    FlintPayload::SensorReading(w) => {
                        info!(
                            "  node={}  {}.{:02}°C  {}%RH  {}×0.5m/s@{}°  \
                             fuel={}%  batt={}%/{}mV  seq={}",
                            w.node_id,
                            w.temp_c / 100,
                            w.temp_c.unsigned_abs() % 100,
                            w.humidity_pct,
                            w.wind_speed_ms,
                            w.wind_dir_deg,
                            w.fuel_moisture,
                            w.battery_soc,
                            w.battery_mv,
                            w.sequence,
                        );
                    }
                }

                // ── Mesh relay ────────────────────────────────────────────────

                if seen.check_and_insert(envelope.packet_id) {
                    match envelope.for_relay() {
                        Some(relay) => {
                            let mut tx_buf = [0u8; MAX_PACKET_BYTES];
                            match encode(&relay, &mut tx_buf) {
                                Ok(bytes) => {
                                    match lora
                                        .prepare_for_tx(&mdm, &mut tx_params, LORA_TX_POWER_DBM, bytes)
                                        .await
                                    {
                                        Ok(_) => match lora.tx().await {
                                            Ok(_) => info!(
                                                "  RELAY id=0x{:08x}  hops_left={}",
                                                relay.packet_id, relay.hop_limit
                                            ),
                                            Err(e) => warn!("tx: {:?}", defmt::Debug2Format(&e)),
                                        },
                                        Err(e) => {
                                            warn!("prepare_for_tx: {:?}", defmt::Debug2Format(&e))
                                        }
                                    }
                                }
                                Err(e) => warn!("encode: {:?}", defmt::Debug2Format(&e)),
                            }
                        }
                        None => info!("  hop_limit=0 — local delivery only"),
                    }
                } else {
                    info!("  duplicate packet_id — discarding");
                }
            }

            Err(_) => {
                // Not a KindlePacket — probably the Arduino default sketch or noise.
                // Hex-dump to see what the transmitter is actually sending.
                let dump = raw.len().min(32);
                info!("  unknown format — first {} bytes:", dump);
                info!("  {:02x}", &raw[..dump]);
            }
        }
    }
}
